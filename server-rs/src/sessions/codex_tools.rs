//! Codex tool calls, read into the shapes Claude's tools already render as.
//!
//! Codex records a tool call in two quite different ways, and which one a
//! rollout uses depends on whether *code mode* is on — which it is for nearly
//! every rollout this machine's CLI has written since 0.148:
//!
//! * **Code mode.** The model has one freeform tool, `exec`, whose input is a
//!   JavaScript cell that calls the real tools — `tools.exec_command(…)`,
//!   `tools.apply_patch(…)`, `tools.mcp__server__tool(…)`, any number of them per
//!   cell. The `custom_tool_call` says nothing structured about what ran, and its
//!   output is whatever the cell printed. What ran is written separately: one
//!   `event_msg` `item_completed` per inner call — `CommandExecution`,
//!   `FileChange`, `McpToolCall`, `WebSearch`, `ImageView`, `Extension` — between
//!   the cell and its output. Those items are the record, and they are what
//!   Codex's own app draws.
//! * **Direct calls.** `function_call` / `custom_tool_call` / `local_shell_call`
//!   (`exec_command`, `shell`, `apply_patch`, `update_plan`, the collaboration
//!   tools…), each answered by an output carrying the same `call_id`. Code-mode
//!   rollouts still use these for the tools that live outside the cell, and a
//!   client that leaves code mode off gets nothing else.
//!
//! Either way what comes out is the pair a Claude `tool_use` makes: a call with
//! `toolUseId` and a result with `resultOf`, named and summarised the way the web
//! already draws Claude's tools. A command reads as what it did — codex's own
//! `parsed_cmd` says "Read ledger.rs" where it can, and the command line says it
//! otherwise — with its working directory as the caption and the exit code and
//! wall time on the result. An edit is an `Edit` / `MultiEdit` / `Write` per
//! file, with the hunks codex recorded as the result's `structuredPatch`. A plan
//! is a checklist.
//!
//! # A cell is the calls inside it
//!
//! A cell's own row can only say "some JavaScript ran", so it is shown for as
//! long as that is all there is to say. It is kept *after* everything the cell
//! has produced so far — always the newest item while the cell runs, which is
//! what keeps the step in flight visible, and means a live delta only ever
//! rewrites the tail — and it leaves when the cell finishes, unless it is the
//! only record of what happened: a cell that called nothing with an item of its
//! own (plain JavaScript, a plan update, a poll of a running process), one that
//! failed, or one still running in the background. A command the cell started
//! and left running has no item yet — it gets one when it finishes, whenever
//! that is — so it takes a row of its own in the cell's place, answered "running
//! in the background" with what it had printed so far; see [`still_running`].
//! Which items a cell made is decided by [`OpenCell::made`], because a command
//! an earlier cell left running finishes while whichever cell is open.
//!
//! # Echoes
//!
//! An `item_completed` is never a second copy of something already drawn, with
//! one exception that is recognised by id: a *direct* call's own item (a
//! `SubAgentActivity`, a `clock.sleep`, an `exec_command`'s `CommandExecution`)
//! carries that call's `call_id`, and the call already has its row — unless the
//! call was answered "still running", when its item is it finishing, and gets a
//! row under `<call_id>#finished`. The older
//! `exec_command_end` / `patch_apply_end` family of `event_msg`s, which echo
//! the response items one for one, is never read at all.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use serde_json::{json, Map, Value};

use super::iso8601;
use super::mentions::resolve_within_home;
use super::tool_display::{as_display_string, compact, diff_of, pick_preferred, pretty, redact_encrypted, shorten_tool_name};
use super::transcript::is_renderable_image;
use super::util::{first_line_of, make_tool_item, ItemKind, PatchHunk, Role, ToolDisplay, TranscriptItem};
use crate::record::replay::parse_iso8601;

type Json = Map<String, Value>;

/// What a command row is called, whichever of codex's several command tools
/// ran it: `exec_command`, `shell`, `shell_command`, `local_shell_call`, or a
/// cell's `CommandExecution`. One name, so a run of them folds under one count.
const SHELL: &str = "shell";
/// A code-mode cell that cannot be described by one tool it called.
const CELL: &str = "js";
/// How far a one-line summary runs before it is ellipsised.
const LINE: usize = 180;

/// Where relative paths resolve and which paths may become thumbnails.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    /// The home directory; nothing outside it can be served as an image.
    pub home: &'a Path,
    /// The session's working directory, from its `session_meta`.
    pub cwd: Option<&'a str>,
}

/// A call row, named and summarised.
fn call(tool: &str, text: impl Into<String>, meta: Option<String>, detail: Option<String>) -> ToolDisplay {
    ToolDisplay {
        tool_name: Some(tool.to_string()),
        text: text.into(),
        meta,
        detail,
        is_error: false,
    }
}

fn str_of<'a>(map: &'a Json, key: &str) -> Option<&'a str> {
    map.get(key).and_then(Value::as_str).filter(|value| !value.trim().is_empty())
}

/// The last component of a path, for a verdict like `updated ledger.rs`.
fn base_name(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path)
}

/// A millisecond stamp from an event, in the transcript's own ISO-8601 form.
fn stamp_of(ms: Option<&Value>) -> Option<String> {
    let ms = ms?.as_u64()?;
    Some(iso8601(std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms)))
}

/// `1.2s`, or `3m 05s` once a call runs past a minute.
fn seconds(wall: f64) -> String {
    if wall < 60.0 {
        format!("{wall:.1}s")
    } else {
        let whole = wall.round() as u64;
        format!("{}m {:02}s", whole / 60, whole % 60)
    }
}

/// Codex's `{secs, nanos}` duration, or a bare millisecond count.
fn duration_of(item: &Json) -> Option<f64> {
    if let Some(Value::Object(duration)) = item.get("duration") {
        let secs = duration.get("secs").and_then(Value::as_f64).unwrap_or(0.0);
        let nanos = duration.get("nanos").and_then(Value::as_f64).unwrap_or(0.0);
        return Some(secs + nanos / 1e9);
    }
    ["durationMs", "duration_ms"]
        .iter()
        .find_map(|key| item.get(*key).and_then(Value::as_f64))
        .map(|ms| ms / 1000.0)
}

/// A `file://` URL as the path it names; anything else as it came.
///
/// Codex writes a `CommandExecution`'s `cwd` and an `ImageView`'s `path` as
/// URLs, percent-encoded — a directory named `api工具` arrives as
/// `api%E5%B7%A5%E5%85%B7`.
fn file_path_of(raw: &str) -> String {
    let Some(rest) = raw.strip_prefix("file://") else {
        return raw.to_string();
    };
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                index += 3;
            }
            None => {
                out.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// An image a result can offer as a thumbnail: one a browser draws, inside
/// the home directory `/api/fs/download` is willing to serve.
fn thumbnail(path: &str, context: Context<'_>) -> Option<Vec<String>> {
    if !is_renderable_image(Path::new(path)) {
        return None;
    }
    let resolved = resolve_within_home(context.home, path, context.cwd.map(Path::new))?;
    Some(vec![resolved.to_string_lossy().into_owned()])
}

// ── commands ──────────────────────────────────────────────────────────────

/// What a shell ran: the script inside `zsh -lc …`, or the argv joined.
fn command_text(command: Option<&Value>) -> String {
    match command {
        Some(Value::String(line)) => line.clone(),
        Some(Value::Array(argv)) => {
            let parts: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
            if let [shell, flag, script] = parts.as_slice() {
                let shell = shell.rsplit('/').next().unwrap_or(shell);
                if matches!(shell, "sh" | "bash" | "zsh" | "dash" | "fish") && flag.starts_with('-') && flag.ends_with('c') {
                    return (*script).to_string();
                }
            }
            parts.iter().map(|part| quote_arg(part)).collect::<Vec<_>>().join(" ")
        }
        _ => String::new(),
    }
}

/// One argv entry as a shell would need it typed.
fn quote_arg(arg: &str) -> String {
    let bare = !arg.is_empty() && arg.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c));
    if bare {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// What codex's own `parsed_cmd` says a command did, when it says it plainly.
///
/// The CLI classifies every command it runs — `read`, `list_files`, `search`,
/// or `unknown` — and its own interface prints the classification instead of
/// the command ("Read ledger.rs"), because `sed -n '1,285p' zhang-core/src/
/// ledger.rs` is the command's business and reading a file is what happened.
/// One `unknown` part and the summary would be a guess, so the command line
/// speaks for itself instead.
fn explored(parsed: &[Value]) -> Option<String> {
    let mut reads: Vec<String> = Vec::new();
    let mut rest: Vec<String> = Vec::new();
    for part in parsed {
        let part = part.as_object()?;
        let path = str_of(part, "path");
        match part.get("type").and_then(Value::as_str)? {
            "read" => {
                let name = str_of(part, "name").or(path.map(base_name))?.to_string();
                if !reads.contains(&name) {
                    reads.push(name);
                }
            }
            "list_files" => rest.push(match path {
                Some(path) => format!("List {path}"),
                None => "List files".to_string(),
            }),
            "search" => rest.push(match (str_of(part, "query"), path) {
                (Some(query), Some(path)) => format!("Search {query} in {path}"),
                (Some(query), None) => format!("Search {query}"),
                (None, Some(path)) => format!("Search {path}"),
                (None, None) => "Search".to_string(),
            }),
            _ => return None,
        }
    }
    let mut parts = Vec::new();
    if !reads.is_empty() {
        parts.push(format!("Read {}", reads.join(", ")));
    }
    parts.extend(rest);
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// A command row: what it did on the line, the directory as the caption, the
/// whole command in the fold.
///
/// Never `$ `-prefixed. That prefix is how the web recognises a Claude `Bash`
/// call that came without a description, and puts the tool name on the row
/// instead — and a codex command never comes with one, so every row of a
/// session read `exec`.
fn shell_call(command: &str, cwd: Option<String>, parsed: Option<&Vec<Value>>) -> ToolDisplay {
    let text = parsed.and_then(|parsed| explored(parsed)).unwrap_or_else(|| {
        let more = if command.trim().contains('\n') { " ⋯" } else { "" };
        format!("{}{more}", first_line_of(command, LINE))
    });
    call(
        SHELL,
        text,
        cwd.filter(|cwd| !cwd.trim().is_empty()),
        (!command.trim().is_empty()).then(|| command.to_string()),
    )
}

/// A `write_stdin` call: a poll when it sends nothing, which it nearly always does.
fn stdin_call(session: Option<String>, chars: Option<String>) -> ToolDisplay {
    let session = session.map(|id| format!("session {id}")).unwrap_or_else(|| "session".to_string());
    let text = match chars.filter(|chars| !chars.is_empty()) {
        Some(chars) => format!("send {} → {session}", serde_json::to_string(&chars).unwrap_or_default()),
        None => format!("poll {session}"),
    };
    call("write_stdin", text, None, None)
}

// ── outputs ───────────────────────────────────────────────────────────────

/// What an output said about how the call went, once its header is read off.
#[derive(Debug, Default)]
struct Outcome {
    body: String,
    exit: Option<i64>,
    /// Seconds the call took, when the output said.
    wall: Option<f64>,
    /// Still going: the process session or cell a later call polls.
    running: Option<String>,
    /// The process session a still-running command answered with, which is
    /// the `process_id` its `CommandExecution` carries when it finishes.
    session: Option<String>,
    /// The cell itself is still running (`Script running with cell ID …`),
    /// rather than some command it started.
    cell_running: bool,
    /// Each command result a cell printed, in the order it printed them.
    printed: Vec<Outcome>,
    failed: bool,
    /// `input_image` parts, which are base64 and never carried.
    images: usize,
    /// The row's line when the body's own first line would say nothing — a
    /// pretty-printed object opens with `{`.
    headline: Option<String>,
}

/// The header a code-mode cell's output opens with.
fn cell_header() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"^Script (completed|failed|running with cell ID (\S+))\nWall time ([\d.]+) seconds\n(?:Output:\n)?")
            .expect("valid cell header")
    })
}

/// Read a direct call's header off its output, when it has one:
///
/// ```text
/// Chunk ID: fdbc3f                  Exit code: 0
/// Wall time: 0.4848 seconds         Wall time: 0 seconds
/// Process exited with code 0        Output:
/// Original token count: 27          Success. Updated the following files: …
/// Output:
/// …
/// ```
///
/// `None` when the first line is not one of these: an output is only a header
/// when it opens with one.
fn shell_header(text: &str) -> Option<Outcome> {
    static WALL: OnceLock<regex::Regex> = OnceLock::new();
    let wall_re = WALL.get_or_init(|| regex::Regex::new(r"^Wall time:? ([\d.]+) seconds?$").expect("valid wall time"));
    let mut outcome = Outcome::default();
    let mut matched = false;
    let mut lines = text.split('\n');
    let mut body: Vec<&str> = Vec::new();
    for line in lines.by_ref() {
        let trimmed = line.trim_end_matches('\r');
        if trimmed == "Output:" {
            break;
        }
        if let Some(code) = trimmed
            .strip_prefix("Process exited with code ")
            .or_else(|| trimmed.strip_prefix("Exit code: "))
        {
            outcome.exit = code.trim().parse().ok();
        } else if let Some(session) = trimmed.strip_prefix("Process running with session ID ") {
            outcome.running = Some(format!("session {}", session.trim()));
            outcome.session = Some(session.trim().to_string());
        } else if let Some(caps) = wall_re.captures(trimmed) {
            outcome.wall = caps[1].parse().ok();
        } else if !(trimmed.starts_with("Chunk ID: ") || trimmed.starts_with("Original token count: ") || trimmed.starts_with("Total output lines: "))
        {
            // The first line that is not a header field is where the body
            // starts — `sleep` writes no `Output:` marker before its sentence.
            if !matched {
                return None;
            }
            body.push(line);
            break;
        }
        matched = true;
    }
    if !matched {
        return None;
    }
    body.extend(lines);
    outcome.body = body.join("\n");
    Some(outcome)
}

/// An exec envelope — `{"chunk_id", "exit_code" | "session_id", "output"}` as
/// code mode returns a command, or `{"output", "metadata": {"exit_code"}}` as
/// the old `shell` tool did — or one of the `Promise.allSettled` wrappers a
/// cell puts around it.
fn envelope(value: &Value) -> Option<Outcome> {
    let object = value.as_object()?;
    for wrapper in ["value", "result"] {
        if let Some(inner) = object
            .get(wrapper)
            .filter(|_| object.contains_key("i") || object.contains_key("index") || object.contains_key("status"))
        {
            return envelope(inner);
        }
    }
    if object.get("status").and_then(Value::as_str) == Some("rejected") {
        let reason = object.get("reason").map(|reason| as_display_string(Some(reason))).unwrap_or_default();
        return Some(Outcome {
            body: reason,
            failed: true,
            ..Outcome::default()
        });
    }
    let output = object.get("output").and_then(Value::as_str)?;
    let metadata = object.get("metadata").and_then(Value::as_object);
    if !(object.contains_key("exit_code") || object.contains_key("chunk_id") || object.contains_key("session_id") || metadata.is_some()) {
        return None;
    }
    let exit = object
        .get("exit_code")
        .or_else(|| metadata.and_then(|meta| meta.get("exit_code")))
        .and_then(Value::as_i64);
    let session = exit
        .is_none()
        .then(|| object.get("session_id").map(|id| as_display_string(Some(id))))
        .flatten();
    Some(Outcome {
        body: output.replace("\r\n", "\n"),
        exit,
        wall: object
            .get("wall_time_seconds")
            .or_else(|| metadata.and_then(|meta| meta.get("duration_seconds")))
            .and_then(Value::as_f64),
        running: session.as_ref().map(|id| format!("session {id}")),
        session,
        ..Outcome::default()
    })
}

/// Unwrap every envelope in one printed part, whole or line by line — a
/// truncated cell prints several on separate lines under a warning.
fn unwrap_part(part: &str, found: &mut Vec<Outcome>) -> String {
    let trimmed = part.trim();
    if trimmed.starts_with('{') {
        if let Some(outcome) = serde_json::from_str::<Value>(trimmed).ok().as_ref().and_then(envelope) {
            let body = outcome.body.clone();
            found.push(outcome);
            return body;
        }
    }
    if !part.contains("\n{") {
        return part.to_string();
    }
    part.split('\n')
        .map(|line| {
            let candidate = line.trim();
            match candidate
                .starts_with('{')
                .then(|| serde_json::from_str::<Value>(candidate).ok())
                .flatten()
                .as_ref()
                .and_then(envelope)
            {
                Some(outcome) => {
                    let body = outcome.body.trim_end().to_string();
                    found.push(outcome);
                    body
                }
                None => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Everything an output can be — a string, a list of `input_text` /
/// `input_image` parts, an object — read down to one [`Outcome`].
fn read_output(output: Option<&Value>) -> Outcome {
    let mut parts: Vec<String> = Vec::new();
    let mut images = 0usize;
    match output {
        Some(Value::String(text)) => {
            let trimmed = text.trim_start();
            match trimmed.starts_with('[').then(|| serde_json::from_str::<Value>(text).ok()).flatten() {
                Some(Value::Array(list)) => collect_parts(&list, &mut parts, &mut images),
                _ => parts.push(text.clone()),
            }
        }
        Some(Value::Array(list)) => collect_parts(list, &mut parts, &mut images),
        Some(Value::Object(map)) => match map.get("content").and_then(Value::as_str) {
            Some(content) => parts.push(content.to_string()),
            None => parts.push(serde_json::to_string_pretty(map).unwrap_or_default()),
        },
        _ => {}
    }

    let mut outcome = Outcome::default();
    // A cell's header is its own first part; what the cell printed follows.
    if let Some(first) = parts.first().cloned() {
        if let Some(caps) = cell_header().captures(&first) {
            outcome.failed = &caps[1] == "failed";
            outcome.running = caps.get(2).map(|cell| format!("cell {}", cell.as_str()));
            outcome.cell_running = outcome.running.is_some();
            outcome.wall = caps[3].parse().ok();
            parts[0] = first[caps[0].len()..].to_string();
            let mut found: Vec<Outcome> = Vec::new();
            let printed: Vec<String> = parts
                .iter()
                .filter(|part| !part.trim().is_empty())
                .map(|part| unwrap_part(part, &mut found))
                .collect();
            outcome.body = printed.join("\n");
            if found.iter().any(|inner| inner.failed) {
                outcome.failed = true;
            }
            // One command's code is the cell's; several have no one code. But
            // any one of them still running is enough for the cell's answer to
            // say so — a fan-out of four where the fourth yielded did not finish.
            if let [only] = found.as_slice() {
                outcome.exit = only.exit;
            }
            if outcome.running.is_none() {
                outcome.running = found.iter().find_map(|inner| inner.running.clone());
            }
            outcome.printed = found;
            outcome.images = images;
            return outcome;
        }
    }

    let text = parts.join("\n");
    let trimmed = text.trim_start();
    if let Some(found) = shell_header(trimmed) {
        outcome = found;
    } else if let Some(found) = trimmed
        .starts_with('{')
        .then(|| serde_json::from_str::<Value>(trimmed).ok())
        .flatten()
        .as_ref()
        .and_then(|value| envelope(value).or_else(|| value.is_object().then(|| object_outcome(value))))
    {
        outcome = found;
    } else {
        outcome.body = text;
    }
    outcome.images = images;
    outcome
}

/// An outcome that is only its text.
fn plain(body: String) -> Outcome {
    Outcome { body, ..Outcome::default() }
}

/// A JSON object answer: pretty in the fold, one compact line on the row.
fn object_outcome(value: &Value) -> Outcome {
    Outcome {
        body: serde_json::to_string_pretty(value).unwrap_or_default(),
        headline: Some(value.to_string()),
        ..Outcome::default()
    }
}

/// The text of every part, in order, and a count of the image parts.
fn collect_parts(list: &[Value], parts: &mut Vec<String>, images: &mut usize) {
    for part in list {
        match part.get("type").and_then(Value::as_str) {
            Some("input_image") | Some("image") => *images += 1,
            _ => {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    parts.push(text.to_string());
                }
            }
        }
    }
}

/// The result row for an outcome: the first line of output, the exit code and
/// wall time as the caption, the whole output in the fold.
///
/// `exit N` in the caption is what the web reads the row's status off, so it
/// is spelled exactly that way and comes first.
fn verdict(outcome: Outcome) -> ToolDisplay {
    let mut body = outcome.body.replace("\r\n", "\n").trim().to_string();
    if outcome.images > 0 {
        let note = format!("[{} image{}]", outcome.images, if outcome.images > 1 { "s" } else { "" });
        body = if body.is_empty() { note } else { format!("{body}\n{note}") };
    }
    let mut meta: Vec<String> = Vec::new();
    if let Some(code) = outcome.exit {
        meta.push(format!("exit {code}"));
    }
    if let Some(running) = &outcome.running {
        meta.push(format!("running · {running}"));
    }
    if let Some(wall) = outcome.wall.filter(|wall| *wall >= 0.1) {
        meta.push(seconds(wall));
    }
    let failed = outcome.failed || outcome.exit.is_some_and(|code| code != 0);
    // Spelled the way Claude's receipt for a `run_in_background` command is,
    // which is what the web already draws as running in the background rather
    // than as `completed`: a command that yielded has not finished, and its
    // first line of output so far says nothing about how it will.
    let text = match &outcome.running {
        Some(running) if outcome.exit.is_none() && !failed => format!("Command running in background ({running})"),
        _ => first_line_of(outcome.headline.as_deref().unwrap_or(&body), LINE),
    };
    ToolDisplay {
        tool_name: None,
        text: if text.is_empty() { "no output".to_string() } else { text },
        meta: (!meta.is_empty()).then(|| meta.join(" · ")),
        detail: (!body.is_empty()).then_some(body),
        is_error: failed,
    }
}

/// The result of any codex call, read from its output alone.
pub fn display_codex_tool_output(output: Option<&Value>) -> ToolDisplay {
    verdict(read_output(output))
}

// ── plans and questions ───────────────────────────────────────────────────

/// One plan step: what, and how far along.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Step {
    text: String,
    status: String,
}

/// `[{step, status}]`, as `update_plan` takes it.
fn steps_of(plan: Option<&Value>) -> Vec<Step> {
    plan.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|step| {
            Some(Step {
                text: str_of(step, "step")?.to_string(),
                status: str_of(step, "status").unwrap_or("pending").to_string(),
            })
        })
        .collect()
}

/// A plan as the checklist Claude's `TodoWrite` already renders: the step in
/// progress on the row, every step marked in the fold.
fn plan_call(steps: &[Step], explanation: Option<String>) -> ToolDisplay {
    let done = steps.iter().filter(|step| step.status == "completed").count();
    let text = match steps.iter().find(|step| matches!(step.status.as_str(), "in_progress" | "inProgress")) {
        Some(current) => format!("{done}/{} · {}", steps.len(), first_line_of(&current.text, LINE)),
        None => format!("{done}/{} steps", steps.len()),
    };
    let lines = steps
        .iter()
        .map(|step| {
            let mark = match step.status.as_str() {
                "completed" => "[x]",
                "in_progress" | "inProgress" => "[~]",
                _ => "[ ]",
            };
            format!("{mark} {}", step.text)
        })
        .collect::<Vec<_>>()
        .join("\n");
    call("update_plan", text, explanation, (!lines.is_empty()).then_some(lines))
}

/// `request_user_input`: the question on the row, every option in the fold,
/// the way an `AskUserQuestion` that has been answered reads.
fn questions_call(name: &str, input: &Json) -> ToolDisplay {
    let questions: Vec<&Json> = input
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .collect();
    let title = |question: &Json| {
        ["question", "title", "header"]
            .iter()
            .find_map(|key| str_of(question, key))
            .unwrap_or("")
            .to_string()
    };
    let mut lines: Vec<String> = Vec::new();
    for question in &questions {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(title(question));
        for option in question.get("options").and_then(Value::as_array).into_iter().flatten() {
            let label = match option {
                Value::Object(option) => str_of(option, "label").unwrap_or("").to_string(),
                other => as_display_string(Some(other)),
            };
            lines.push(format!("◦ {label}"));
        }
    }
    call(
        name,
        questions
            .first()
            .map(|question| first_line_of(&title(question), LINE))
            .unwrap_or_default(),
        (questions.len() > 1).then(|| format!("{} questions", questions.len())),
        (!lines.is_empty()).then(|| lines.join("\n")),
    )
}

// ── patches ───────────────────────────────────────────────────────────────

/// What one patch did to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FileOp {
    Add {
        content: String,
    },
    Delete {
        content: Option<String>,
    },
    Update {
        move_to: Option<String>,
        /// Each hunk as the text it replaced and the text it left, which is
        /// what Claude's `Edit` and `MultiEdit` carry.
        edits: Vec<(String, String)>,
        /// The hunks with their line numbers, when codex recorded a real diff.
        hunks: Option<Vec<PatchHunk>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatchFile {
    path: String,
    op: FileOp,
}

/// Old and new text for one hunk's `' '` / `-` / `+` rows.
fn edit_of(rows: &[&str]) -> (String, String) {
    let mut old: Vec<&str> = Vec::new();
    let mut new: Vec<&str> = Vec::new();
    for row in rows {
        match row.chars().next() {
            Some('-') => old.push(&row[1..]),
            Some('+') => new.push(&row[1..]),
            Some(' ') => {
                old.push(&row[1..]);
                new.push(&row[1..]);
            }
            // A blank row is a blank context line whose space was trimmed.
            None => {
                old.push("");
                new.push("");
            }
            _ => {}
        }
    }
    (old.join("\n"), new.join("\n"))
}

/// The files an `apply_patch` envelope touches, and what it does to each:
///
/// ```text
/// *** Begin Patch
/// *** Update File: src/a.rs
/// *** Move to: src/b.rs
/// @@ fn main
///  context
/// -old
/// +new
/// *** Add File: src/c.rs
/// +content
/// *** Delete File: src/d.rs
/// *** End Patch
/// ```
fn parse_envelope(patch: &str) -> Vec<PatchFile> {
    fn finish(files: &mut Vec<PatchFile>, current: Option<(String, Vec<&str>, Option<String>, char)>) {
        let Some((path, rows, move_to, kind)) = current else { return };
        let op = match kind {
            'A' => FileOp::Add {
                content: rows.iter().map(|row| row.strip_prefix('+').unwrap_or(row)).collect::<Vec<_>>().join("\n"),
            },
            'D' => FileOp::Delete { content: None },
            _ => {
                let mut edits = Vec::new();
                for hunk in rows.split(|row| row.starts_with("@@")) {
                    if hunk.iter().any(|row| row.starts_with('-') || row.starts_with('+')) {
                        edits.push(edit_of(hunk));
                    }
                }
                FileOp::Update { move_to, edits, hunks: None }
            }
        };
        files.push(PatchFile { path, op });
    }

    let mut files = Vec::new();
    let mut current: Option<(String, Vec<&str>, Option<String>, char)> = None;
    for line in patch.lines() {
        let started = [("*** Add File: ", 'A'), ("*** Delete File: ", 'D'), ("*** Update File: ", 'U')]
            .iter()
            .find_map(|(marker, kind)| line.strip_prefix(marker).map(|path| (path.trim().to_string(), *kind)));
        if let Some((path, kind)) = started {
            finish(&mut files, current.take());
            current = Some((path, Vec::new(), None, kind));
            continue;
        }
        if let Some(to) = line.strip_prefix("*** Move to: ") {
            if let Some(file) = current.as_mut() {
                file.2 = Some(to.trim().to_string());
            }
            continue;
        }
        if line.starts_with("*** ") {
            // `Begin Patch`, `End Patch`, `End of File`: punctuation.
            continue;
        }
        if let Some(file) = current.as_mut() {
            file.1.push(line);
        }
    }
    finish(&mut files, current);
    files
}

/// A unified diff's hunks, with line numbers, and as old/new text pairs.
fn parse_unified(diff: &str) -> (Vec<PatchHunk>, Vec<(String, String)>) {
    static HEADER: OnceLock<regex::Regex> = OnceLock::new();
    let header = HEADER.get_or_init(|| regex::Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").expect("valid hunk header"));
    let mut hunks: Vec<PatchHunk> = Vec::new();
    for line in diff.lines() {
        if let Some(caps) = header.captures(line) {
            let number = |index: usize, default: u32| caps.get(index).and_then(|m| m.as_str().parse().ok()).unwrap_or(default);
            hunks.push(PatchHunk {
                old_start: number(1, 0),
                old_lines: number(2, 1),
                new_start: number(3, 0),
                new_lines: number(4, 1),
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = hunks.last_mut() else { continue };
        match line.chars().next() {
            Some('+') | Some('-') | Some(' ') => hunk.lines.push(line.to_string()),
            // `\ No newline at end of file` says nothing a reader needs.
            Some('\\') => {}
            None => hunk.lines.push(" ".to_string()),
            _ => {}
        }
    }
    hunks.retain(|hunk| !hunk.lines.is_empty());
    let edits = hunks
        .iter()
        .map(|hunk| edit_of(&hunk.lines.iter().map(String::as_str).collect::<Vec<_>>()))
        .collect();
    (hunks, edits)
}

/// A `FileChange` item's `changes` map, one entry per file.
fn file_changes(changes: Option<&Value>) -> Vec<PatchFile> {
    let Some(changes) = changes.and_then(Value::as_object) else {
        return Vec::new();
    };
    changes
        .iter()
        .filter_map(|(path, change)| {
            let change = change.as_object()?;
            let content = || change.get("content").and_then(Value::as_str).map(str::to_owned);
            let op = match change.get("type").and_then(Value::as_str)? {
                "add" => FileOp::Add {
                    content: content().unwrap_or_default(),
                },
                "delete" => FileOp::Delete { content: content() },
                _ => {
                    let (hunks, edits) = parse_unified(change.get("unified_diff").and_then(Value::as_str).unwrap_or(""));
                    FileOp::Update {
                        move_to: str_of(change, "move_path").map(str::to_owned),
                        edits,
                        hunks: (!hunks.is_empty()).then_some(hunks),
                    }
                }
            };
            Some(PatchFile { path: path.clone(), op })
        })
        .collect()
}

/// One file of a patch, as the Claude tool that would have made the same
/// change: `Write` for a new file, `Edit` for one hunk, `MultiEdit` for several.
fn file_call(file: &PatchFile) -> ToolDisplay {
    match &file.op {
        FileOp::Add { content } => call("Write", file.path.clone(), None, Some(content.clone())),
        FileOp::Delete { content } => call("Delete", file.path.clone(), None, content.clone().filter(|c| !c.is_empty())),
        FileOp::Update { move_to, edits, .. } => {
            let text = match move_to {
                Some(to) => format!("{} → {to}", file.path),
                None => file.path.clone(),
            };
            match edits.as_slice() {
                [] => call("Edit", text, None, None),
                [(old, new)] => call("Edit", text, None, Some(diff_of(old, new))),
                many => {
                    let edits: Vec<Value> = many.iter().map(|(old, new)| json!({"old_string": old, "new_string": new})).collect();
                    let input = json!({"file_path": file.path, "edits": edits});
                    call("MultiEdit", text, None, input.as_object().map(pretty))
                }
            }
        }
    }
}

/// How one file of a patch went: `updated ledger.rs`, or the error.
fn file_result(file: &PatchFile, failure: Option<&str>) -> ToolDisplay {
    if let Some(failure) = failure {
        let failure = failure.trim();
        return ToolDisplay {
            tool_name: None,
            text: if failure.is_empty() {
                "failed".to_string()
            } else {
                first_line_of(failure, LINE)
            },
            meta: None,
            detail: (!failure.is_empty()).then(|| failure.to_string()),
            is_error: true,
        };
    }
    let verb = match &file.op {
        FileOp::Add { .. } => "created",
        FileOp::Delete { .. } => "deleted",
        FileOp::Update { move_to: Some(_), .. } => "moved",
        FileOp::Update { .. } => "updated",
    };
    ToolDisplay {
        tool_name: None,
        text: format!("{verb} {}", base_name(&file.path)),
        meta: None,
        detail: None,
        is_error: false,
    }
}

/// The hunks a direct `apply_patch`'s own `FileChange` recorded for a file
/// the envelope named relative to the working directory.
fn echoed_hunks(echo: &[PatchFile], path: &str) -> Option<Vec<PatchHunk>> {
    let suffix = format!("/{}", path.trim_start_matches("./"));
    echo.iter()
        .find(|file| file.path == path || file.path.ends_with(&suffix))
        .and_then(|file| match &file.op {
            FileOp::Update { hunks, .. } => hunks.clone(),
            _ => None,
        })
}

/// The files an `apply_patch` envelope touches, as a one-line summary.
fn patch_file_list(patch: &str) -> String {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"^\*\*\* (Add|Update|Delete) File: (.+)$").expect("valid patch header"));
    let files: Vec<String> = patch
        .split('\n')
        .filter_map(|line| re.captures(line))
        .map(|caps| format!("{} {}", caps[1].to_lowercase(), &caps[2]))
        .collect();
    if files.is_empty() {
        "patch".to_string()
    } else {
        files.join(", ")
    }
}

fn is_patch(input: &str) -> bool {
    input.trim_start().starts_with("*** Begin Patch")
}

// ── JavaScript cells ──────────────────────────────────────────────────────

/// The source text of the balanced `{...}` starting at byte `start`.
///
/// Codex logs the *call site*, not the arguments, so an object has to be cut
/// out of JavaScript source — which means tracking string literals, or a brace
/// inside `cmd: "echo }"` would end the object early.
fn balanced_object(src: &str, start: usize) -> Option<String> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut quote = ' ';
    for (index, ch) in src.get(start..)?.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' | '\'' | '`' => {
                in_string = true;
                quote = ch;
            }
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(src[start..start + index + ch.len_utf8()].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// A JavaScript string literal's value.
fn unquote(literal: &str) -> Option<String> {
    if literal.len() < 2 {
        return None;
    }
    if literal.starts_with('"') {
        if let Ok(value) = serde_json::from_str::<String>(literal) {
            return Some(value);
        }
    }
    // A single- or backtick-quoted literal is not JSON, so it is unescaped by
    // hand — the same three escapes the TypeScript knows about.
    let inner = &literal[1..literal.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    Some(out)
}

/// The string literal opening at `start` (after whitespace), if one does.
fn literal_at(src: &str, start: usize) -> Option<String> {
    let rest = src.get(start..)?.trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\'' | '`'))?;
    let mut escaped = false;
    for (index, ch) in rest.char_indices().skip(1) {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            return unquote(&rest[..index + 1]);
        }
    }
    None
}

/// Pull one string-valued field out of a JS object literal.
///
/// Codex emits both JSON-style (`{"cmd": "…"}`) and bare-key JS (`{cmd: "…"}`)
/// argument objects, so parsing as JSON alone is not enough.
fn extract_string_field(obj_src: &str, key: &str) -> Option<String> {
    field_literal(obj_src, key, r#"("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|`(?:[^`\\]|\\.)*`)"#).and_then(|literal| unquote(&literal))
}

/// A numeric field, which a JS object writes bare: `{session_id: 34720}`.
fn extract_number_field(obj_src: &str, key: &str) -> Option<String> {
    field_literal(obj_src, key, r"(-?\d+(?:\.\d+)?)")
}

fn field_literal(obj_src: &str, key: &str, value: &str) -> Option<String> {
    // The pattern depends on the key, so it cannot be a `OnceLock` constant;
    // compiled patterns are memoised instead, since a transcript asks for the
    // same few keys on every call.
    static CACHE: OnceLock<std::sync::Mutex<HashMap<String, regex::Regex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let pattern = {
        let mut cache = cache.lock().expect("field pattern cache poisoned");
        cache
            .entry(format!("{key}\u{0}{value}"))
            .or_insert_with(|| {
                let key = regex::escape(key);
                regex::Regex::new(&format!(r#"(?:^|[{{,\s])(?:"{key}"|'{key}'|{key})\s*:\s*{value}"#)).expect("valid field pattern")
            })
            .clone()
    };
    Some(pattern.captures(obj_src)?.get(1)?.as_str().to_string())
}

/// One `tools.<name>(…)` in a cell, with its argument object when it has one.
struct InnerCall {
    name: String,
    /// Byte offset just past the opening parenthesis.
    args_at: usize,
    object: Option<String>,
}

/// Every `tools.<name>(…)` a cell makes, in the order it makes them.
fn inner_calls(source: &str) -> Vec<InnerCall> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"\btools\.([A-Za-z_][A-Za-z0-9_]*)\s*\(").expect("valid tool call pattern"));
    re.captures_iter(source)
        .map(|caps| {
            let whole = caps.get(0).expect("match");
            let args_at = whole.end();
            let rest = &source[args_at..];
            let object = rest
                .trim_start()
                .starts_with('{')
                .then(|| balanced_object(source, args_at + (rest.len() - rest.trim_start().len())))
                .flatten();
            InnerCall {
                name: caps[1].to_string(),
                args_at,
                object,
            }
        })
        .collect()
}

/// The plan a cell hands `update_plan`, read out of its object literals —
/// `{ step: "…", status: "in_progress" }` — wherever in the cell they are,
/// since the model usually builds the array in a variable first.
fn plan_in_source(source: &str) -> Vec<Step> {
    static OBJECT: OnceLock<regex::Regex> = OnceLock::new();
    let object = OBJECT.get_or_init(|| regex::Regex::new(r"\{[^{}]*\}").expect("valid object pattern"));
    object
        .find_iter(source)
        .filter_map(|found| {
            let text = extract_string_field(found.as_str(), "step")?;
            Some(Step {
                text,
                status: extract_string_field(found.as_str(), "status").unwrap_or_else(|| "pending".to_string()),
            })
        })
        .collect()
}

/// The first line of a cell that says something: not blank, not a comment.
fn first_statement(source: &str) -> String {
    let line = source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("//"))
        .unwrap_or("");
    first_line_of(line, LINE)
}

/// What a code-mode cell's own row says.
///
/// A cell that makes one call is named after it — a lone `exec_command` is a
/// command, a lone `write_stdin` a poll — because that is the row it stands
/// for until the call's own item arrives, and the only one if none ever does.
/// Anything else is `js`, summarised by the calls it makes or, with none, by
/// its first statement; the source is the fold either way.
fn cell_call(source: &str) -> ToolDisplay {
    let calls = inner_calls(source);
    if calls.iter().any(|inner| inner.name == "update_plan") {
        let steps = plan_in_source(source);
        if !steps.is_empty() {
            return plan_call(&steps, None);
        }
    }
    if let [only] = calls.as_slice() {
        let object = only.object.as_deref().unwrap_or("");
        let field = |key: &str| extract_string_field(object, key);
        let named = match only.name.as_str() {
            "exec_command" => field("cmd").map(|cmd| shell_call(&cmd, field("workdir"), None)),
            "write_stdin" => Some(stdin_call(extract_number_field(object, "session_id"), field("chars"))),
            "view_image" => field("path").map(|path| call("view_image", path, None, None)),
            "apply_patch" => literal_at(source, only.args_at)
                .filter(|patch| is_patch(patch))
                .map(|patch| call("apply_patch", patch_file_list(&patch), None, Some(patch))),
            "web__run" => field("q").map(|query| call("WebSearch", query, None, Some(source.to_string()))),
            name if name.starts_with("mcp__") => Some(call(
                &shorten_tool_name(name),
                ["title", "query", "text", "path", "url"]
                    .iter()
                    .find_map(|key| field(key))
                    .map(|text| first_line_of(&text, LINE))
                    .unwrap_or_default(),
                None,
                Some(source.to_string()),
            )),
            _ => None,
        };
        if let Some(named) = named {
            return named;
        }
    }
    let text = if calls.is_empty() {
        first_statement(source)
    } else {
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for inner in &calls {
            match counts.iter_mut().find(|(name, _)| *name == inner.name) {
                Some((_, n)) => *n += 1,
                None => counts.push((&inner.name, 1)),
            }
        }
        counts
            .iter()
            .map(|(name, n)| if *n > 1 { format!("{name} ×{n}") } else { name.to_string() })
            .collect::<Vec<_>>()
            .join(" · ")
    };
    call(CELL, text, None, Some(source.to_string()))
}

// ── direct calls ──────────────────────────────────────────────────────────

/// `30s`, from a millisecond argument.
fn millis_text(input: &Json, key: &str) -> Option<String> {
    input.get(key).and_then(Value::as_f64).map(|ms| seconds(ms / 1000.0))
}

/// A `custom_tool_call`'s row: a patch envelope, a code-mode cell, or a
/// freeform tool nothing here knows, shown by its first line.
pub fn display_codex_custom_tool_call(name: &str, input: &str) -> ToolDisplay {
    if is_patch(input) {
        return call("apply_patch", patch_file_list(input), None, Some(input.to_string()));
    }
    if name == "exec" {
        return cell_call(input);
    }
    call(name, first_line_of(input, LINE), None, Some(input.to_string()))
}

/// A `function_call`: `arguments` is a JSON-encoded string, and `namespace`
/// names the family a newer CLI files a tool under (`collaboration`, `clock`,
/// or an MCP server).
pub fn display_codex_function_call(name: &str, namespace: Option<&str>, args: &str) -> ToolDisplay {
    let Ok(Value::Object(input)) = serde_json::from_str::<Value>(args) else {
        return call(name, first_line_of(args, LINE), None, Some(args.to_string()));
    };
    let field = |key: &str| str_of(&input, key).map(str::to_owned);
    match name {
        "exec_command" | "shell_command" | "shell" | "container.exec" | "local_shell" => {
            let command = command_text(input.get("cmd").or_else(|| input.get("command")));
            return shell_call(&command, field("workdir").or_else(|| field("working_directory")), None);
        }
        "write_stdin" => {
            let session = input.get("session_id").map(|id| as_display_string(Some(id)));
            return stdin_call(session, field("chars"));
        }
        "update_plan" => {
            let steps = steps_of(input.get("plan"));
            if !steps.is_empty() {
                return plan_call(&steps, field("explanation"));
            }
        }
        "view_image" => return call("view_image", field("path").unwrap_or_default(), None, None),
        "request_user_input" | "request_user_input_async" => return questions_call(name, &input),
        "sleep" => return call(name, millis_text(&input, "duration_ms").unwrap_or_default(), None, None),
        "wait_agent" => return call(name, millis_text(&input, "timeout_ms").unwrap_or_default(), None, None),
        "wait" if input.contains_key("cell_id") => {
            return call(name, format!("cell {}", as_display_string(input.get("cell_id"))), None, None);
        }
        "spawn_agent" | "send_message" | "followup_task" | "send_input" => {
            // The message itself is usually a fernet blob, so who it went to is
            // the readable part of the call.
            let redacted = redact_encrypted(&input);
            let text = match (field("task_name").or_else(|| field("agent_type")), field("target")) {
                (Some(task), _) => task,
                (None, Some(target)) => format!("→ {target}"),
                (None, None) => str_of(&redacted, "message").map(|m| first_line_of(m, LINE)).unwrap_or_default(),
            };
            return call(name, text, None, Some(pretty(&redacted)));
        }
        _ => {}
    }
    // An MCP tool, under whichever spelling the CLI used: `mcp__server__tool`,
    // `server__tool`, or `tool` in an `mcp__server__` namespace.
    let tool_name = match namespace.filter(|space| space.starts_with("mcp__")) {
        Some(space) => format!("{}:{name}", space.trim_start_matches("mcp__").trim_end_matches('_')),
        None if name.starts_with("mcp__") => shorten_tool_name(name),
        None => match name.split_once("__") {
            Some((server, tool)) if !server.is_empty() && !tool.is_empty() => format!("{server}:{tool}"),
            _ => name.to_string(),
        },
    };
    if input.is_empty() {
        return call(&tool_name, String::new(), None, None);
    }
    let redacted = redact_encrypted(&input);
    call(
        &tool_name,
        first_line_of(&pick_preferred(&redacted).unwrap_or_else(|| compact(&redacted)), LINE),
        field("task_name"),
        Some(pretty(&redacted)),
    )
}

// ── items a cell produced ─────────────────────────────────────────────────

/// One call and its result, out of an `item_completed`.
struct Pair {
    id: String,
    call: ToolDisplay,
    result: ToolDisplay,
    patch: Option<Vec<PatchHunk>>,
    images: Option<Vec<String>>,
}

impl Pair {
    fn new(id: String, call: ToolDisplay, result: ToolDisplay) -> Self {
        Self {
            id,
            call,
            result,
            patch: None,
            images: None,
        }
    }
}

/// The id a patch gives its `n`th file: the call's own for the first, so the
/// common one-file patch keeps the id the CLI wrote.
fn file_id(id: &str, n: usize) -> String {
    if n == 0 {
        id.to_string()
    } else {
        format!("{id}#{}", n + 1)
    }
}

/// An MCP result's text, or its error.
fn mcp_result(item: &Json) -> ToolDisplay {
    let wall = duration_of(item);
    if let Some(error) = item.get("error").filter(|error| !error.is_null()) {
        let message = match error {
            Value::Object(error) => str_of(error, "message").map(str::to_owned).unwrap_or_else(|| compact(error)),
            other => as_display_string(Some(other)),
        };
        return verdict(Outcome {
            body: message,
            wall,
            failed: true,
            ..Outcome::default()
        });
    }
    let result = item.get("result").and_then(Value::as_object);
    let mut parts: Vec<String> = Vec::new();
    let mut images = 0usize;
    if let Some(content) = result.and_then(|result| result.get("content")).and_then(Value::as_array) {
        collect_parts(content, &mut parts, &mut images);
    }
    verdict(Outcome {
        body: parts.join("\n"),
        wall,
        failed: result.and_then(|result| result.get("isError")) == Some(&Value::Bool(true))
            || item.get("status").and_then(Value::as_str) == Some("failed"),
        images,
        ..Outcome::default()
    })
}

/// A web search, page open or find-in-page, as Claude's `WebSearch` /
/// `WebFetch` would read.
fn web_pair(id: String, item: &Json) -> Pair {
    let action = item.get("action").and_then(Value::as_object).cloned().unwrap_or_default();
    let query = str_of(item, "query").map(str::to_owned);
    let url = str_of(&action, "url").map(str::to_owned);
    let (tool, text) = match action.get("type").and_then(Value::as_str).unwrap_or("") {
        "open_page" | "openPage" => ("WebFetch", url.or(query).unwrap_or_default()),
        "find_in_page" | "findInPage" => {
            let pattern = str_of(&action, "pattern").unwrap_or("");
            (
                "WebFetch",
                match url {
                    Some(url) => format!("find “{pattern}” in {url}"),
                    None => format!("find “{pattern}”"),
                },
            )
        }
        _ => {
            let queries: Vec<String> = action
                .get("queries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            let text = if queries.is_empty() {
                str_of(&action, "query").map(str::to_owned).or(query).unwrap_or_default()
            } else {
                queries.join(" · ")
            };
            ("WebSearch", text)
        }
    };
    let results: Vec<&Json> = item
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .collect();
    // A search is a list of hits; an opened page or a find is what it read.
    let reading = tool == "WebFetch";
    let lines: Vec<String> = results
        .iter()
        .map(|result| {
            let title = str_of(result, "title").unwrap_or("");
            let head = match str_of(result, "url") {
                Some(url) if !title.is_empty() => format!("{} — {url}", first_line_of(title, LINE)),
                Some(url) => url.to_string(),
                None => first_line_of(title, LINE),
            };
            match str_of(result, "snippet").filter(|_| reading) {
                Some(snippet) if head.is_empty() => snippet.trim().to_string(),
                Some(snippet) => format!("{head}\n{}", snippet.trim()),
                None => head,
            }
        })
        .filter(|line| !line.is_empty())
        .collect();
    let result = ToolDisplay {
        tool_name: None,
        text: match (results.len(), reading) {
            (0, _) => "no results".to_string(),
            (_, true) => lines
                .first()
                .map(|line| first_line_of(line, LINE))
                .unwrap_or_else(|| "opened".to_string()),
            (1, false) => "1 result".to_string(),
            (n, false) => format!("{n} results"),
        },
        meta: None,
        detail: (!lines.is_empty()).then(|| lines.join(if reading { "\n\n" } else { "\n" })),
        is_error: false,
    };
    Pair::new(id, call(tool, first_line_of(&text, LINE), None, None), result)
}

/// The calls and results one `item_completed` stands for — nothing for an
/// item that is not a tool call (messages, reasoning, sub-agent activity).
fn item_pairs(item: &Json, context: Context<'_>) -> Vec<Pair> {
    let id = str_of(item, "id").unwrap_or("item").to_string();
    let status = item.get("status").and_then(Value::as_str);
    let failed_status = matches!(status, Some("failed") | Some("declined") | Some("rejected"));
    match item.get("type").and_then(Value::as_str).unwrap_or("") {
        "CommandExecution" => {
            let command = command_text(item.get("command"));
            let cwd = str_of(item, "cwd").map(file_path_of);
            let call = shell_call(&command, cwd, item.get("parsed_cmd").and_then(Value::as_array));
            let output = ["aggregated_output", "formatted_output"]
                .iter()
                .find_map(|key| item.get(*key).and_then(Value::as_str).filter(|out| !out.is_empty()))
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    [str_of(item, "stdout"), str_of(item, "stderr")]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join("\n")
                });
            let result = verdict(Outcome {
                body: output,
                exit: item.get("exit_code").and_then(Value::as_i64),
                wall: duration_of(item),
                failed: failed_status,
                ..Outcome::default()
            });
            vec![Pair::new(id, call, result)]
        }
        "FileChange" => {
            let failure = failed_status.then(|| {
                [str_of(item, "stderr"), str_of(item, "stdout"), status]
                    .into_iter()
                    .flatten()
                    .next()
                    .unwrap_or("failed")
                    .to_string()
            });
            file_changes(item.get("changes"))
                .into_iter()
                .enumerate()
                .map(|(n, file)| {
                    let mut pair = Pair::new(file_id(&id, n), file_call(&file), file_result(&file, failure.as_deref()));
                    if let FileOp::Update { hunks, .. } = &file.op {
                        pair.patch = hunks.clone();
                    }
                    pair
                })
                .collect()
        }
        "McpToolCall" => {
            let server = str_of(item, "server").unwrap_or("mcp");
            let tool = str_of(item, "tool").unwrap_or("tool");
            let args = item.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default();
            let redacted = redact_encrypted(&args);
            // `node_repl`'s whole argument is a program; it reads as one.
            let detail = match str_of(&args, "code") {
                Some(code) => code.to_string(),
                None => pretty(&redacted),
            };
            let text = if args.is_empty() {
                String::new()
            } else {
                first_line_of(&pick_preferred(&redacted).unwrap_or_else(|| compact(&redacted)), LINE)
            };
            let call = call(&format!("{server}:{tool}"), text, None, (!args.is_empty()).then_some(detail));
            vec![Pair::new(id, call, mcp_result(item))]
        }
        "WebSearch" => vec![web_pair(id, item)],
        "Extension" => match item.get("kind").and_then(Value::as_str) {
            Some("web.search") => vec![web_pair(id, item)],
            Some("image_gen.generation") => {
                let prompt = str_of(item, "revisedPrompt").unwrap_or("").to_string();
                let saved = str_of(item, "savedPath").map(str::to_owned);
                let failure = item
                    .get("failure")
                    .filter(|failure| !failure.is_null())
                    .map(|f| as_display_string(Some(f)));
                let result = match (&failure, &saved) {
                    (Some(failure), _) => verdict(Outcome {
                        body: failure.clone(),
                        failed: true,
                        ..Outcome::default()
                    }),
                    (None, Some(saved)) => verdict(plain(format!("generated {}", base_name(saved)))),
                    (None, None) => verdict(Outcome {
                        failed: failed_status,
                        ..Outcome::default()
                    }),
                };
                let mut pair = Pair::new(id, call("image_gen", first_line_of(&prompt, LINE), None, Some(prompt)), result);
                // Never `result`: it is the image itself, base64, megabytes of it.
                pair.images = saved.as_deref().and_then(|saved| thumbnail(saved, context));
                vec![pair]
            }
            // `clock.sleep` and anything newer: the direct call it belongs to
            // already has a row, or there is nothing to say about it.
            _ => Vec::new(),
        },
        "ImageView" => {
            let path = str_of(item, "path").map(file_path_of).unwrap_or_default();
            let mut pair = Pair::new(
                id,
                call("view_image", path.clone(), None, None),
                verdict(plain(format!("viewed {}", base_name(&path)))),
            );
            pair.images = thumbnail(&path, context);
            vec![pair]
        }
        _ => Vec::new(),
    }
}

// ── the transcript pass ───────────────────────────────────────────────────

/// What a call left to be answered.
#[derive(Debug)]
enum Pending {
    /// One row; its result inherits `tool_name`, and a `view_image` offers its
    /// picture.
    Single { tool_name: Option<String>, image: Option<String> },
    /// An `apply_patch`, one row per file.
    Patch(Vec<PatchFile>),
}

/// The cell whose output has not arrived yet.
#[derive(Debug)]
struct OpenCell {
    call_id: String,
    /// Where its own row is in the items: always after what it produced.
    index: usize,
    /// How many inner calls it has produced rows for.
    inner: usize,
    /// When it was called, in epoch milliseconds; see [`OpenCell::made`].
    called_ms: Option<i64>,
    /// The tools its source calls; see [`OpenCell::made`].
    tools: Vec<String>,
    /// Its source, for naming the commands still running when it returns.
    source: String,
    /// What its own `CommandExecution`s ran.
    done: Vec<String>,
}

/// Pairs codex's calls with their results across one pass over a rollout.
///
/// Every call is matched to its output by `call_id`, which is what makes the
/// web draw them as one row. Code-mode cells are folded into the calls they
/// make; see the module docs.
#[derive(Debug, Default)]
pub struct CodexTools {
    pending: HashMap<String, Pending>,
    cell: Option<OpenCell>,
    /// A direct `apply_patch`'s own `FileChange`, which knows the line numbers
    /// the envelope does not.
    echoes: HashMap<String, Vec<PatchFile>>,
    /// Direct calls already answered, and whether the answer said the call was
    /// still running — so that call's own item, arriving after the answer, is
    /// either its finish or an echo, and never a second row under its id.
    answered: HashMap<String, bool>,
}

impl CodexTools {
    /// Nothing pending: the start of a rollout.
    pub fn new() -> Self {
        Self::default()
    }

    /// A `function_call`, `custom_tool_call` or `local_shell_call`.
    pub fn call(&mut self, items: &mut Vec<TranscriptItem>, payload: &Json, timestamp: Option<&str>, model: Option<&str>) {
        let call_id = str_of(payload, "call_id").or_else(|| str_of(payload, "id")).map(str::to_owned);
        let name = str_of(payload, "name").unwrap_or("tool");
        let display = match payload.get("type").and_then(Value::as_str) {
            Some("custom_tool_call") => {
                let input = payload.get("input").and_then(Value::as_str).unwrap_or("");
                if is_patch(input) {
                    if self.patch_rows(items, call_id.as_deref(), input, timestamp, model) {
                        return;
                    }
                } else if name == "exec" {
                    let display = cell_call(input);
                    if let Some(id) = &call_id {
                        self.cell = Some(OpenCell {
                            call_id: id.clone(),
                            index: items.len(),
                            inner: 0,
                            called_ms: timestamp.and_then(parse_iso8601),
                            tools: inner_calls(input).into_iter().map(|inner| inner.name).collect(),
                            source: input.to_string(),
                            done: Vec::new(),
                        });
                        self.pending.insert(
                            id.clone(),
                            Pending::Single {
                                tool_name: display.tool_name.clone(),
                                image: None,
                            },
                        );
                    }
                    push_call(items, display, call_id, timestamp, model);
                    return;
                }
                display_codex_custom_tool_call(name, input)
            }
            Some("local_shell_call") => {
                let action = payload.get("action").and_then(Value::as_object).cloned().unwrap_or_default();
                let command = command_text(action.get("command"));
                shell_call(&command, str_of(&action, "working_directory").map(str::to_owned), None)
            }
            _ => {
                let args = payload.get("arguments").and_then(Value::as_str).unwrap_or("{}");
                if name == "apply_patch" {
                    let envelope = serde_json::from_str::<Value>(args).ok().and_then(|args| {
                        ["input", "patch"]
                            .iter()
                            .find_map(|key| args.get(*key).and_then(Value::as_str).map(str::to_owned))
                    });
                    if let Some(envelope) = envelope.filter(|envelope| is_patch(envelope)) {
                        if self.patch_rows(items, call_id.as_deref(), &envelope, timestamp, model) {
                            return;
                        }
                    }
                }
                display_codex_function_call(name, payload.get("namespace").and_then(Value::as_str), args)
            }
        };
        if let Some(id) = &call_id {
            let image = (display.tool_name.as_deref() == Some("view_image")).then(|| display.text.clone());
            self.pending.insert(
                id.clone(),
                Pending::Single {
                    tool_name: display.tool_name.clone(),
                    image,
                },
            );
        }
        push_call(items, display, call_id, timestamp, model);
    }

    /// One row per file an `apply_patch` envelope touches; `false` when it
    /// names none, and the caller shows the envelope as it is.
    fn patch_rows(
        &mut self,
        items: &mut Vec<TranscriptItem>,
        call_id: Option<&str>,
        envelope: &str,
        timestamp: Option<&str>,
        model: Option<&str>,
    ) -> bool {
        let files = parse_envelope(envelope);
        if files.is_empty() {
            return false;
        }
        for (n, file) in files.iter().enumerate() {
            push_call(items, file_call(file), call_id.map(|id| file_id(id, n)), timestamp, model);
        }
        if let Some(id) = call_id {
            self.pending.insert(id.to_string(), Pending::Patch(files));
        }
        true
    }

    /// A `function_call_output` or `custom_tool_call_output`.
    pub fn output(&mut self, items: &mut Vec<TranscriptItem>, payload: &Json, timestamp: Option<&str>, context: Context<'_>) {
        let call_id = str_of(payload, "call_id").map(str::to_owned);
        let outcome = read_output(payload.get("output"));

        if let Some(cell) = self.cell.take_if(|cell| Some(&cell.call_id) == call_id.as_ref()) {
            if is_last_row(items, &cell) && !outcome.failed && !outcome.cell_running {
                // Its inner calls, and the commands it left running, said
                // everything a finished cell has to say. Only ever the last
                // item goes, so nothing a client holds moves.
                let yielded = outcome.printed.iter().filter(|printed| printed.session.is_some()).count();
                let running = match yielded {
                    0 => Some(Vec::new()),
                    n => still_running(&cell.source, &cell.done, n),
                };
                if let Some(running) = running.filter(|running| cell.inner + running.len() > 0) {
                    self.pending.remove(&cell.call_id);
                    let own = items.pop().expect("the cell's row is the last item");
                    let printed = outcome.printed.into_iter().filter(|printed| printed.session.is_some());
                    for ((command, workdir), printed) in running.into_iter().zip(printed) {
                        let id = format!("{}#{}", cell.call_id, printed.session.as_deref().unwrap_or_default());
                        push_call(
                            items,
                            shell_call(&command, workdir, None),
                            Some(id.clone()),
                            own.timestamp.0.as_deref(),
                            own.model.as_deref(),
                        );
                        let mut result = verdict(printed);
                        result.tool_name = Some(SHELL.to_string());
                        let mut item = make_tool_item(Role::User, ItemKind::ToolResult, result, timestamp);
                        item.result_of = Some(id);
                        items.push(item);
                    }
                    return;
                }
            }
        }
        if let Some(id) = &call_id {
            self.answered.insert(id.clone(), outcome.exit.is_none() && outcome.session.is_some());
        }

        match call_id.as_ref().and_then(|id| self.pending.remove(id)) {
            Some(Pending::Patch(files)) => {
                let id = call_id.unwrap_or_default();
                let echo = self.echoes.remove(&id).unwrap_or_default();
                let display = verdict(outcome);
                let failed = display.is_error
                    || (display.meta.is_none() && {
                        let head = display.text.to_lowercase();
                        head.contains("error") || head.contains("failed")
                    });
                let failure = failed.then(|| display.detail.clone().unwrap_or_else(|| display.text.clone()));
                for (n, file) in files.iter().enumerate() {
                    let mut result = file_result(file, failure.as_deref());
                    result.tool_name = file_call(file).tool_name;
                    let mut item = make_tool_item(Role::User, ItemKind::ToolResult, result, timestamp);
                    item.result_of = Some(file_id(&id, n));
                    item.structured_patch = match &file.op {
                        FileOp::Update { .. } if !failed => echoed_hunks(&echo, &file.path),
                        _ => None,
                    };
                    items.push(item);
                }
            }
            pending => {
                let (tool_name, image) = match pending {
                    Some(Pending::Single { tool_name, image }) => (tool_name, image),
                    _ => (None, None),
                };
                let mut display = verdict(outcome);
                display.tool_name = tool_name;
                let mut item = make_tool_item(Role::User, ItemKind::ToolResult, display, timestamp);
                item.images = image.and_then(|path| thumbnail(&path, context));
                item.result_of = call_id;
                items.push(item);
            }
        }
    }

    /// An `event_msg` `item_completed`: the record of a call a cell made.
    pub fn item_completed(
        &mut self,
        items: &mut Vec<TranscriptItem>,
        event: &Json,
        timestamp: Option<&str>,
        model: Option<&str>,
        context: Context<'_>,
    ) {
        let Some(item) = event.get("item").and_then(Value::as_object) else {
            return;
        };
        let mut renamed: Option<Json> = None;
        if let Some(id) = str_of(item, "id") {
            if let Some(pending) = self.pending.get(id) {
                // A direct call's own item: the call already has its row. A
                // patch's is worth keeping for the line numbers it carries.
                if matches!(pending, Pending::Patch(_)) && item.get("type").and_then(Value::as_str) == Some("FileChange") {
                    self.echoes.insert(id.to_string(), file_changes(item.get("changes")));
                }
                return;
            }
            match self.answered.get_mut(id) {
                // Answered "still running": this is it finishing, which is news,
                // under an id of its own beside the call's.
                Some(running) if *running => {
                    *running = false;
                    let mut finished = item.clone();
                    finished.insert("id".to_string(), Value::String(format!("{id}#finished")));
                    renamed = Some(finished);
                }
                // Answered already: an echo that lost the race to the answer.
                Some(_) => return,
                None => {}
            }
        }
        let item = renamed.as_ref().unwrap_or(item);
        let pairs = item_pairs(item, context);
        if pairs.is_empty() {
            return;
        }
        let started_ms = event.get("started_at_ms").and_then(Value::as_i64);
        let ours = self.cell.as_ref().is_some_and(|cell| cell.made(item, started_ms));
        let ran = (ours && item.get("type").and_then(Value::as_str) == Some("CommandExecution")).then(|| command_text(item.get("command")));
        let started = stamp_of(event.get("started_at_ms")).or_else(|| timestamp.map(str::to_owned));
        let finished = stamp_of(event.get("completed_at_ms")).or_else(|| timestamp.map(str::to_owned));
        let mut rows: Vec<TranscriptItem> = Vec::with_capacity(pairs.len() * 2);
        for pair in pairs {
            let tool_name = pair.call.tool_name.clone();
            let mut use_item = make_tool_item(Role::Assistant, ItemKind::ToolUse, pair.call, started.as_deref());
            use_item.tool_use_id = Some(pair.id.clone());
            use_item.model = model.map(str::to_owned);
            let mut result = pair.result;
            result.tool_name = tool_name;
            let mut result_item = make_tool_item(Role::User, ItemKind::ToolResult, result, finished.as_deref());
            result_item.result_of = Some(pair.id);
            result_item.structured_patch = pair.patch;
            result_item.images = pair.images;
            rows.push(use_item);
            rows.push(result_item);
        }
        match self.cell.as_mut().filter(|cell| is_last_row(items, cell)) {
            // Ahead of the cell's own row, which stays the newest item.
            Some(cell) => {
                let count = rows.len();
                items.splice(cell.index..cell.index, rows);
                cell.index += count;
                if ours {
                    cell.inner += 1;
                    cell.done.extend(ran);
                }
            }
            // No cell, or one that something else has since been written
            // after — an interrupted turn's, whose output is never coming.
            // What finishes now is news in its own right, at the end.
            None => {
                self.cell = None;
                items.extend(rows);
            }
        }
    }

    /// A `web_search_call` response item: the hosted search, which has no output
    /// of its own to pair with.
    pub fn web_search_call(&mut self, items: &mut Vec<TranscriptItem>, payload: &Json, timestamp: Option<&str>, model: Option<&str>) {
        // Pairing is by id, so a search the CLI wrote without one still gets
        // one of its own.
        let id = str_of(payload, "id")
            .or_else(|| str_of(payload, "call_id"))
            .map(str::to_owned)
            .unwrap_or_else(|| format!("web_search@{}", items.len()));
        let pair = web_pair(id, payload);
        let tool_name = pair.call.tool_name.clone();
        push_call(items, pair.call, Some(pair.id.clone()), timestamp, model);
        if let Some(status) = str_of(payload, "status").filter(|status| *status != "in_progress") {
            let result = ToolDisplay {
                tool_name,
                text: status.to_string(),
                meta: None,
                detail: None,
                is_error: matches!(status, "failed" | "incomplete"),
            };
            let mut item = make_tool_item(Role::User, ItemKind::ToolResult, result, timestamp);
            item.result_of = Some(pair.id);
            items.push(item);
        }
    }
}

impl OpenCell {
    /// Whether this cell made the call an `item_completed` reports, rather than
    /// an earlier call finishing while it happens to be open.
    ///
    /// A command an earlier cell left running completes whenever it completes,
    /// often while a later cell — a `write_stdin` poll, a plan update — is
    /// open. Counting it as that cell's would have the cell's own row taken for
    /// redundant and dropped, and with it the only record of what the poll saw.
    /// Two tests tell them apart, both measured on every rollout on this
    /// machine:
    ///
    /// * **When it started.** The event's `started_at_ms` is when the *call*
    ///   started. All 6 336 items a cell made started at or after the cell was
    ///   called; all 522 that started before it were an earlier call's, nearly
    ///   all a session an earlier cell had already reported running.
    /// * **What the cell calls.** Each kind of item comes from one tool — a
    ///   `CommandExecution` from `exec_command`, a `FileChange` from
    ///   `apply_patch` — and every item a cell made came from a tool its source
    ///   names. A cell that only updates a plan did not run a command.
    ///
    /// Either test failing keeps the cell's row, which costs a row and never
    /// loses one.
    fn made(&self, item: &Json, started_ms: Option<i64>) -> bool {
        if let (Some(called), Some(started)) = (self.called_ms, started_ms) {
            if started < called {
                return false;
            }
        }
        let calls = |prefix: &str| self.tools.iter().any(|name| name.starts_with(prefix));
        match item.get("type").and_then(Value::as_str).unwrap_or("") {
            "CommandExecution" => calls("exec_command"),
            "FileChange" => calls("apply_patch"),
            "ImageView" => calls("view_image"),
            "McpToolCall" => calls("mcp__"),
            "WebSearch" => calls("web__"),
            "Extension" => match item.get("kind").and_then(Value::as_str) {
                Some("web.search") => calls("web__"),
                Some("image_gen.generation") => calls("image_gen"),
                _ => true,
            },
            _ => true,
        }
    }
}

/// The commands a cell started and returned before they finished, in the
/// order it started them, each with its working directory — `None` when that
/// cannot be said for certain.
///
/// A command that yields prints an envelope with a session id and no command,
/// so the command is found by elimination: every `exec_command` in the source,
/// less the ones whose `CommandExecution` came back while the cell ran. Only an
/// exact count is trusted. A command built in a variable or a loop, or a
/// `write_stdin` poll of something an earlier call started, leaves the cell's
/// own row to say it instead of a guess.
fn still_running(source: &str, done: &[String], count: usize) -> Option<Vec<(String, Option<String>)>> {
    let mut left = inner_calls(source)
        .into_iter()
        .filter(|inner| inner.name == "exec_command")
        .map(|inner| {
            let object = inner.object?;
            Some((extract_string_field(&object, "cmd")?, extract_string_field(&object, "workdir")))
        })
        .collect::<Option<Vec<_>>>()?;
    for command in done {
        if let Some(index) = left.iter().position(|(left, _)| left == command) {
            left.remove(index);
        }
    }
    (left.len() == count).then_some(left)
}

/// Whether the open cell's own row is still the newest item.
fn is_last_row(items: &[TranscriptItem], cell: &OpenCell) -> bool {
    cell.index + 1 == items.len() && items[cell.index].tool_use_id.as_deref() == Some(cell.call_id.as_str())
}

fn push_call(items: &mut Vec<TranscriptItem>, display: ToolDisplay, id: Option<String>, timestamp: Option<&str>, model: Option<&str>) {
    let mut item = make_tool_item(Role::Assistant, ItemKind::ToolUse, display, timestamp);
    item.tool_use_id = id;
    item.model = model.map(str::to_owned);
    items.push(item);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::Paths;
    use serde_json::json;

    /// One rollout line: `{"timestamp", "type", "payload"}`.
    fn line(kind: &str, payload: Value) -> String {
        json!({"timestamp": "2026-10-04T14:55:07.760Z", "type": kind, "payload": payload}).to_string()
    }

    /// A code-mode cell, as `custom_tool_call` `exec` writes it.
    fn cell(call_id: &str, source: &str) -> String {
        line(
            "response_item",
            json!({"type": "custom_tool_call", "status": "completed", "call_id": call_id, "name": "exec", "input": source}),
        )
    }

    /// The cell's output: the header part, then whatever it printed.
    fn cell_output(call_id: &str, header: &str, printed: &[&str]) -> String {
        let mut parts = vec![json!({"type": "input_text", "text": header})];
        parts.extend(printed.iter().map(|text| json!({"type": "input_text", "text": text})));
        line(
            "response_item",
            json!({"type": "custom_tool_call_output", "call_id": call_id, "output": parts}),
        )
    }

    /// An `item_completed` event carrying `item`.
    fn completed(item: Value) -> String {
        completed_from(item, 1_791_125_707_887)
    }

    /// An `item_completed` for a call that started at `started_ms`. Every
    /// fixture line is stamped `14:55:07.760`, so anything earlier than that
    /// started before the cell around it.
    fn completed_from(item: Value, started_ms: u64) -> String {
        line(
            "event_msg",
            json!({"type": "item_completed", "thread_id": "t", "turn_id": "u", "item": item,
                   "started_at_ms": started_ms, "completed_at_ms": 1_791_125_708_387u64}),
        )
    }

    fn command(id: &str, script: &str, exit: i64, output: &str, parsed: Value) -> Value {
        json!({
            "type": "CommandExecution", "id": id, "process_id": "98684",
            "command": ["/bin/zsh", "-lc", script],
            "cwd": "file:///Users/me/proj%20%E5%B7%A5%E5%85%B7",
            "parsed_cmd": parsed, "source": "unified_exec_startup",
            "status": if exit == 0 { "completed" } else { "failed" },
            "stdout": output, "stderr": "", "aggregated_output": output,
            "exit_code": exit, "duration": {"secs": 0, "nanos": 412_000_000}, "formatted_output": output,
        })
    }

    async fn parse_in(home: &Path, lines: &[String]) -> Vec<TranscriptItem> {
        let path = home.join("rollout-2026-10-04T23-54-59-11111111-2222-3333-4444-555555555555.jsonl");
        let mut body = vec![line("session_meta", json!({"id": "x", "cwd": "/Users/me/proj"}))];
        body.extend(lines.iter().cloned());
        std::fs::write(&path, body.join("\n") + "\n").expect("rollout");
        crate::sessions::codex::parse_codex_transcript(&Paths::new(home.to_path_buf()), &path)
            .await
            .expect("parse")
    }

    async fn parse(lines: &[String]) -> Vec<TranscriptItem> {
        let home = tempfile::tempdir().expect("tempdir");
        parse_in(home.path(), lines).await
    }

    fn shape(items: &[TranscriptItem]) -> Vec<(ItemKind, Option<&str>, &str)> {
        items
            .iter()
            .map(|item| (item.kind, item.tool_name.0.as_deref(), item.text.as_str()))
            .collect()
    }

    // ── code mode ─────────────────────────────────────────────────────────

    /// The shape of nearly every codex tool call on this machine: one cell, two
    /// commands, each recorded as its own `CommandExecution`. The cell's row
    /// said `exec`; what ran was `pwd` and a file read.
    #[tokio::test]
    async fn a_cell_reads_as_the_commands_it_ran() {
        let items = parse(&[
            cell("call_A", "const r = await Promise.allSettled([\n  tools.exec_command({cmd:\"pwd\"}),\n  tools.exec_command({cmd:\"cat SKILL.md\"}),\n]);\nr.forEach((r,i)=>text({i,...r}));"),
            completed(command("exec-1", "pwd", 0, "/Users/me/proj\n", json!([{"type": "unknown", "cmd": "pwd"}]))),
            completed(command(
                "exec-2",
                "cat SKILL.md",
                0,
                "---\nname: skill\n",
                json!([{"type": "read", "cmd": "cat SKILL.md", "name": "SKILL.md", "path": "SKILL.md"}]),
            )),
            cell_output(
                "call_A",
                "Script completed\nWall time 0.1 seconds\nOutput:\n",
                &[r#"{"i":0,"status":"fulfilled","value":{"chunk_id":"172c74","wall_time_seconds":0.000006,"exit_code":0,"original_token_count":8,"output":"/Users/me/proj\n"}}"#],
            ),
        ])
        .await;

        assert_eq!(
            shape(&items),
            vec![
                (ItemKind::ToolUse, Some("shell"), "pwd"),
                (ItemKind::ToolResult, Some("shell"), "/Users/me/proj"),
                (ItemKind::ToolUse, Some("shell"), "Read SKILL.md"),
                (ItemKind::ToolResult, Some("shell"), "---"),
            ],
            "the cell's own row leaves once its calls have said everything"
        );
        assert_eq!(items[0].tool_use_id.as_deref(), Some("exec-1"));
        assert_eq!(items[1].result_of.as_deref(), Some("exec-1"), "each result answers its own call");
        assert_eq!(items[0].meta.0.as_deref(), Some("/Users/me/proj 工具"), "the cwd URL is decoded");
        assert_eq!(items[2].detail.0.as_deref(), Some("cat SKILL.md"), "the command itself is in the fold");
        assert_eq!(items[1].meta.0.as_deref(), Some("exit 0 · 0.4s"));
        assert_eq!(
            items[0].timestamp.0.as_deref(),
            Some("2026-10-04T14:55:07.887Z"),
            "when the command started"
        );
        assert_eq!(items[1].timestamp.0.as_deref(), Some("2026-10-04T14:55:08.387Z"), "and when it finished");
        assert!(!items[0].text.starts_with("$ "), "a `$ ` row would be drawn as just the tool name");
    }

    #[tokio::test]
    async fn a_failed_command_carries_its_exit_code() {
        let items = parse(&[
            cell("call_A", "text(await tools.exec_command({cmd:\"git show HEAD:nope\"}))"),
            completed(command(
                "exec-1",
                "git show HEAD:nope",
                128,
                "fatal: path 'nope' does not exist in 'HEAD'\n",
                json!([{"type": "unknown", "cmd": "git show HEAD:nope"}]),
            )),
            cell_output(
                "call_A",
                "Script completed\nWall time 0.0 seconds\nOutput:\n",
                &[r#"{"chunk_id":"a","wall_time_seconds":0.01,"exit_code":128,"original_token_count":9,"output":"fatal: path 'nope' does not exist in 'HEAD'\n"}"#],
            ),
        ])
        .await;
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].meta.0.as_deref(), Some("exit 128 · 0.4s"));
        assert!(items[1].is_error);
    }

    /// While a cell runs its row is the newest item — the step in flight — and
    /// what it has finished so far lands above it.
    #[tokio::test]
    async fn a_running_cell_stays_below_what_it_has_done_so_far() {
        let items = parse(&[
            cell(
                "call_A",
                "tools.exec_command({cmd:\"cargo build\"});\ntools.exec_command({cmd:\"cargo test\"});",
            ),
            completed(command(
                "exec-1",
                "cargo build",
                0,
                "Finished\n",
                json!([{"type": "unknown", "cmd": "cargo build"}]),
            )),
        ])
        .await;
        assert_eq!(
            shape(&items),
            vec![
                (ItemKind::ToolUse, Some("shell"), "cargo build"),
                (ItemKind::ToolResult, Some("shell"), "Finished"),
                (ItemKind::ToolUse, Some("js"), "exec_command ×2"),
            ]
        );
        assert_eq!(items[2].tool_use_id.as_deref(), Some("call_A"));
        assert!(
            items[2].detail.0.as_deref().is_some_and(|source| source.contains("cargo test")),
            "the source is the fold"
        );
    }

    /// A cell cut off by an interrupted turn never gets its output. What
    /// finishes after the user has spoken again lands at the end, not tucked in
    /// above a row from several minutes ago.
    #[tokio::test]
    async fn an_abandoned_cell_does_not_pull_later_rows_above_it() {
        let items = parse(&[
            cell("call_A", "tools.exec_command({cmd:\"sleep 600\"});"),
            line("event_msg", json!({"type": "turn_aborted", "reason": "interrupted"})),
            line(
                "response_item",
                json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "stop"}]}),
            ),
            completed(command("exec-1", "sleep 600", 130, "", json!([{"type": "unknown", "cmd": "sleep 600"}]))),
        ])
        .await;
        assert_eq!(
            shape(&items),
            vec![
                (ItemKind::ToolUse, Some("shell"), "sleep 600"),
                (ItemKind::Text, None, "stop"),
                (ItemKind::ToolUse, Some("shell"), "sleep 600"),
                (ItemKind::ToolResult, Some("shell"), "no output"),
            ]
        );
    }

    /// Plain JavaScript has no inner item to stand for it, so the cell is the row.
    #[tokio::test]
    async fn a_cell_that_called_nothing_is_its_own_row() {
        let items = parse(&[
            cell("call_A", "// split the bill\nconst names = [\"a\", \"b\"];\ntext({total: 4});"),
            cell_output("call_A", "Script completed\nWall time 0.0 seconds\nOutput:\n", &[r#"{"total":4}"#]),
        ])
        .await;
        assert_eq!(
            shape(&items),
            vec![
                (ItemKind::ToolUse, Some("js"), "const names = [\"a\", \"b\"];"),
                (ItemKind::ToolResult, Some("js"), "{\"total\":4}"),
            ]
        );
        assert_eq!(items[1].result_of.as_deref(), Some("call_A"));
    }

    #[tokio::test]
    async fn a_cell_that_failed_keeps_its_row_after_its_calls() {
        let items = parse(&[
            cell("call_A", "await tools.exec_command({cmd:\"ls\"});\nthrow new Error(\"boom\");"),
            completed(command(
                "exec-1",
                "ls",
                0,
                "a.txt\n",
                json!([{"type": "list_files", "cmd": "ls", "path": null}]),
            )),
            cell_output("call_A", "Script failed\nWall time 0.2 seconds\nOutput:\n", &["Error: boom"]),
        ])
        .await;
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].text, "List files");
        assert_eq!((items[2].tool_name.0.as_deref(), items[3].text.as_str()), (Some("shell"), "Error: boom"));
        assert!(items[3].is_error, "the failure is the one thing only the cell knows");
        assert_eq!(items[3].meta.0.as_deref(), Some("0.2s"));
    }

    /// A command that outlives its cell answers with a session id; its
    /// `CommandExecution` arrives later, outside any cell, and is its own row.
    #[tokio::test]
    async fn a_yielded_command_finishes_as_a_row_of_its_own() {
        let items = parse(&[
            cell(
                "call_A",
                "text(await tools.exec_command({cmd:\"python build.py\",workdir:\"/w\",yield_time_ms:1000}))",
            ),
            cell_output(
                "call_A",
                "Script completed\nWall time 1.4 seconds\nOutput:\n",
                &[r#"{"chunk_id":"6da718","wall_time_seconds":1.0,"session_id":26552,"original_token_count":0,"output":""}"#],
            ),
            completed(command(
                "exec-9",
                "python build.py",
                0,
                "Pages rendered\n",
                json!([{"type": "unknown", "cmd": "python build.py"}]),
            )),
        ])
        .await;
        assert_eq!(
            shape(&items),
            vec![
                (ItemKind::ToolUse, Some("shell"), "python build.py"),
                (ItemKind::ToolResult, Some("shell"), "Command running in background (session 26552)"),
                (ItemKind::ToolUse, Some("shell"), "python build.py"),
                (ItemKind::ToolResult, Some("shell"), "Pages rendered"),
            ]
        );
        assert_eq!(items[0].tool_use_id.as_deref(), Some("call_A#26552"));
        assert!(!items[1].is_error, "running is not failing");
        assert_eq!(items[0].meta.0.as_deref(), Some("/w"));
        assert_eq!(
            items[1].meta.0.as_deref(),
            Some("running · session 26552 · 1.0s"),
            "the command's own wall time, not the cell's"
        );
    }

    /// A fan-out where one command finished and one yielded: the cell's row is
    /// not needed, but the yielded command is, with what it printed so far.
    #[tokio::test]
    async fn a_command_left_running_beside_a_finished_one_keeps_a_row() {
        let items = parse(&[
            cell(
                "call_A",
                "const r = await Promise.allSettled([\n  tools.exec_command({cmd:\"sed -n 1p a\"}),\n  tools.exec_command({cmd:\"uv run long.py\",workdir:\"/w\",yield_time_ms:1000}),\n]);\nr.forEach((r,i)=>text({i,...r}));",
            ),
            completed(command("exec-1", "sed -n 1p a", 0, "x\n", json!([{"type": "unknown", "cmd": "sed -n 1p a"}]))),
            cell_output(
                "call_A",
                "Script completed\nWall time 1.2 seconds\nOutput:\n",
                &[
                    r#"{"i":0,"status":"fulfilled","value":{"chunk_id":"1","wall_time_seconds":0.0,"exit_code":0,"original_token_count":1,"output":"x\n"}}"#,
                    r#"{"i":1,"status":"fulfilled","value":{"chunk_id":"2","wall_time_seconds":1.0,"session_id":87142,"original_token_count":1,"output":"Downloading torch\n"}}"#,
                ],
            ),
        ])
        .await;
        assert_eq!(
            shape(&items),
            vec![
                (ItemKind::ToolUse, Some("shell"), "sed -n 1p a"),
                (ItemKind::ToolResult, Some("shell"), "x"),
                (ItemKind::ToolUse, Some("shell"), "uv run long.py"),
                (ItemKind::ToolResult, Some("shell"), "Command running in background (session 87142)"),
            ]
        );
        assert_eq!(items[2].meta.0.as_deref(), Some("/w"));
        assert_eq!(
            items[3].detail.0.as_deref(),
            Some("Downloading torch"),
            "what it printed before it yielded"
        );
        assert_eq!(items[3].meta.0.as_deref(), Some("running · session 87142 · 1.0s"));
        assert_eq!(items[3].result_of, items[2].tool_use_id);
    }

    /// When which command yielded cannot be told — a command in a variable, a
    /// poll of an earlier one — the cell's row stays and says it is running.
    #[tokio::test]
    async fn a_yield_that_cannot_be_named_keeps_the_cells_row() {
        let items = parse(&[
            cell(
                "call_A",
                "await tools.exec_command({cmd:\"ls\"});\nfor (const cmd of todo) text(await tools.exec_command({cmd, yield_time_ms:1000}));",
            ),
            completed(command("exec-1", "ls", 0, "a\n", json!([{"type": "unknown", "cmd": "ls"}]))),
            cell_output(
                "call_A",
                "Script completed\nWall time 1.2 seconds\nOutput:\n",
                &[r#"{"chunk_id":"2","wall_time_seconds":1.0,"session_id":87142,"original_token_count":1,"output":"building\n"}"#],
            ),
        ])
        .await;
        assert_eq!(shape(&items)[2], (ItemKind::ToolUse, Some("js"), "exec_command ×2"));
        assert_eq!(items[3].text, "Command running in background (session 87142)");
        assert_eq!(items[3].detail.0.as_deref(), Some("building"));
    }

    /// A command an earlier cell left running finishes while a poll cell is
    /// open. It started before the poll did, so it is not the poll's own call,
    /// and the poll — the only record of what it saw — keeps its row.
    #[tokio::test]
    async fn an_earlier_call_finishing_inside_a_cell_is_not_that_cells() {
        let items = parse(&[
            cell(
                "call_B",
                "const plan = [{step:\"build\",status:\"in_progress\"}];\ntools.update_plan({plan});",
            ),
            completed_from(
                command(
                    "exec-9",
                    "python old.py",
                    0,
                    "done\n",
                    json!([{"type": "unknown", "cmd": "python old.py"}]),
                ),
                1_791_125_600_000,
            ),
            cell_output("call_B", "Script completed\nWall time 0.0 seconds\nOutput:\n", &[]),
        ])
        .await;
        assert_eq!(
            shape(&items),
            vec![
                (ItemKind::ToolUse, Some("shell"), "python old.py"),
                (ItemKind::ToolResult, Some("shell"), "done"),
                (ItemKind::ToolUse, Some("update_plan"), "0/1 · build"),
                (ItemKind::ToolResult, Some("update_plan"), "no output"),
            ],
            "the finish lands above the open cell, which stays and keeps its answer"
        );
    }

    /// A direct command that yielded, whose own item arrives after its answer:
    /// that item is the command finishing, and gets a row — under an id of its
    /// own, never a second call under the first one's.
    #[tokio::test]
    async fn a_direct_command_finishing_after_its_answer_is_one_more_row() {
        let items = parse(&[
            function_call("call_X", "exec_command", json!({"cmd": "sleep 30"})),
            function_output(
                "call_X",
                json!("Chunk ID: ab\nWall time: 10.0 seconds\nProcess running with session ID 5\nOriginal token count: 0\nOutput:\n"),
            ),
            completed(command("call_X", "sleep 30", 0, "", json!([{"type": "unknown", "cmd": "sleep 30"}]))),
            completed(command("call_X", "sleep 30", 0, "", json!([{"type": "unknown", "cmd": "sleep 30"}]))),
        ])
        .await;
        assert_eq!(
            shape(&items),
            vec![
                (ItemKind::ToolUse, Some("shell"), "sleep 30"),
                (ItemKind::ToolResult, Some("shell"), "Command running in background (session 5)"),
                (ItemKind::ToolUse, Some("shell"), "sleep 30"),
                (ItemKind::ToolResult, Some("shell"), "no output"),
            ],
            "and a repeat of the finish is not a third"
        );
        let calls: Vec<_> = items.iter().filter_map(|item| item.tool_use_id.as_deref()).collect();
        assert_eq!(calls, vec!["call_X", "call_X#finished"]);
        assert_eq!(items[3].meta.0.as_deref(), Some("exit 0 · 0.4s"));
    }

    /// The same item after an answer that said the call had finished is an
    /// echo that lost the race, and adds nothing.
    #[tokio::test]
    async fn an_echo_after_its_answer_is_not_a_second_row() {
        let items = parse(&[
            function_call("call_X", "exec_command", json!({"cmd": "pwd"})),
            function_output(
                "call_X",
                json!("Chunk ID: ab\nWall time: 0.0 seconds\nProcess exited with code 0\nOutput:\n/w\n"),
            ),
            completed(command("call_X", "pwd", 0, "/w\n", json!([{"type": "unknown", "cmd": "pwd"}]))),
        ])
        .await;
        assert_eq!(items.len(), 2);
    }

    /// One `FileChange` touching two files is two rows, each the Claude tool
    /// that would have made the same change, and the update carries codex's
    /// own hunks — real line numbers — as its `structuredPatch`.
    #[tokio::test]
    async fn a_file_change_is_a_diff_per_file() {
        let items = parse(&[
            cell("call_A", "text(await tools.apply_patch(\"*** Begin Patch\\n...\"))"),
            completed(json!({
                "type": "FileChange", "id": "exec-7",
                "changes": {
                    "/w/build.py": {"type": "update", "move_path": null,
                        "unified_diff": "@@ -19,3 +19,3 @@\n sec.header = 1\n-FONT = 'Songti SC'\n+FONT = 'Noto Sans CJK SC'\n \n@@ -40,2 +40,3 @@\n a\n+b\n c\n"},
                    "/w/fonts.conf": {"type": "add", "content": "<fontconfig/>\n"},
                },
                "status": "completed",
                "stdout": "Success. Updated the following files:\nA /w/fonts.conf\nM /w/build.py\n", "stderr": "",
            })),
            cell_output("call_A", "Script completed\nWall time 0.0 seconds\nOutput:\n", &["{}"]),
        ])
        .await;
        assert_eq!(
            shape(&items),
            vec![
                (ItemKind::ToolUse, Some("MultiEdit"), "/w/build.py"),
                (ItemKind::ToolResult, Some("MultiEdit"), "updated build.py"),
                (ItemKind::ToolUse, Some("Write"), "/w/fonts.conf"),
                (ItemKind::ToolResult, Some("Write"), "created fonts.conf"),
            ]
        );
        let hunks = items[1].structured_patch.as_ref().expect("hunks");
        assert_eq!((hunks[0].old_start, hunks[0].new_start, hunks[0].lines.len()), (19, 19, 4));
        assert_eq!(hunks[1].lines, vec![" a", "+b", " c"]);
        let detail: Value = serde_json::from_str(items[0].detail.0.as_deref().expect("detail")).expect("MultiEdit's own JSON");
        assert_eq!(detail["edits"][0]["old_string"], "sec.header = 1\nFONT = 'Songti SC'\n");
        assert_eq!(detail["edits"][0]["new_string"], "sec.header = 1\nFONT = 'Noto Sans CJK SC'\n");
        assert_eq!(items[2].detail.0.as_deref(), Some("<fontconfig/>\n"), "a new file's content is its diff");
        assert_eq!(items[2].tool_use_id.as_deref(), Some("exec-7#2"), "each file is its own pair");
        assert_eq!(items[3].result_of.as_deref(), Some("exec-7#2"));
    }

    #[tokio::test]
    async fn an_mcp_call_is_named_by_server_and_tool() {
        let items = parse(&[
            cell("call_A", "text(await tools.mcp__banana_memory__recall({query:\"rollout format\"}))"),
            completed(json!({
                "type": "McpToolCall", "id": "exec-3", "server": "banana-memory", "tool": "recall",
                "arguments": {"query": "rollout format", "projectName": "remote-hand"}, "readOnlyHint": true,
                "status": "completed", "result": {"content": [{"type": "text", "text": "{\"memories\":[]}"}], "isError": false},
                "duration": {"secs": 0, "nanos": 599_387_000},
            })),
            completed(json!({
                "type": "McpToolCall", "id": "exec-4", "server": "node_repl", "tool": "js",
                "arguments": {"title": "Inspect the page", "code": "await page.title()"},
                "status": "failed", "error": {"message": "timeoutReached"}, "duration": {"secs": 30, "nanos": 0},
            })),
            cell_output("call_A", "Script completed\nWall time 0.6 seconds\nOutput:\n", &["{\"memories\":[]}"]),
        ])
        .await;
        assert_eq!(
            shape(&items),
            vec![
                (ItemKind::ToolUse, Some("banana-memory:recall"), "rollout format"),
                (ItemKind::ToolResult, Some("banana-memory:recall"), "{\"memories\":[]}"),
                (ItemKind::ToolUse, Some("node_repl:js"), "Inspect the page"),
                (ItemKind::ToolResult, Some("node_repl:js"), "timeoutReached"),
            ]
        );
        assert_eq!(items[1].meta.0.as_deref(), Some("0.6s"));
        assert_eq!(
            items[2].detail.0.as_deref(),
            Some("await page.title()"),
            "a program reads as one, not as escaped JSON"
        );
        assert!(items[3].is_error);
    }

    #[tokio::test]
    async fn web_searches_and_page_opens_read_like_claudes() {
        let items = parse(&[
            cell("call_A", "text(await tools.web__run({search_query:[{q:\"CNY JPY rate\"}]}))"),
            completed(json!({
                "type": "Extension", "kind": "web.search", "id": "exec-5", "query": "CNY JPY rate",
                "action": {"type": "search", "query": null, "queries": ["CNY JPY rate", "yuan yen"]},
                "results": [
                    {"type": "text_result", "domain": "wise.com", "ref_id": "turn0search0", "snippet": "…", "title": "CNY to JPY", "url": "https://wise.com/cny-jpy"},
                    {"type": "text_result", "domain": "xe.com", "ref_id": "turn0search1", "snippet": "…", "title": "1 CNY to JPY", "url": "https://xe.com/cny"},
                ],
            })),
            completed(json!({
                "type": "WebSearch", "id": "exec-6", "query": "https://github.com/o/r/releases",
                "action": {"type": "open_page", "url": "https://github.com/o/r/releases"},
                "results": [{"type": "text_result", "ref_id": "turn0view0", "snippet": "v0.2.90", "title": "Releases"}],
            })),
            cell_output("call_A", "Script completed\nWall time 2.3 seconds\nOutput:\n", &["CNY to JPY (https://wise.com/cny-jpy)"]),
        ])
        .await;
        assert_eq!(
            shape(&items),
            vec![
                (ItemKind::ToolUse, Some("WebSearch"), "CNY JPY rate · yuan yen"),
                (ItemKind::ToolResult, Some("WebSearch"), "2 results"),
                (ItemKind::ToolUse, Some("WebFetch"), "https://github.com/o/r/releases"),
                (ItemKind::ToolResult, Some("WebFetch"), "Releases"),
            ]
        );
        assert_eq!(
            items[1].detail.0.as_deref(),
            Some("CNY to JPY — https://wise.com/cny-jpy\n1 CNY to JPY — https://xe.com/cny")
        );
    }

    /// The picture codex looked at is offered as a thumbnail, from a
    /// percent-encoded `file://` URL — never from the base64 the cell's output
    /// carried.
    #[tokio::test]
    async fn a_viewed_image_becomes_a_thumbnail() {
        let home = tempfile::tempdir().expect("tempdir");
        let picture = home.path().join("render 工具").join("page-1.png");
        let url = format!(
            "file://{}",
            picture.to_string_lossy().replace(' ', "%20").replace("工具", "%E5%B7%A5%E5%85%B7")
        );
        let items = parse_in(
            home.path(),
            &[
                cell("call_A", "await tools.view_image({path:\"render/page-1.png\"})"),
                completed(json!({"type": "ImageView", "id": "exec-8", "path": url})),
                line(
                    "response_item",
                    json!({"type": "custom_tool_call_output", "call_id": "call_A", "output": [
                        {"type": "input_text", "text": "Script completed\nWall time 0.0 seconds\nOutput:\n"},
                        {"type": "input_image", "image_url": "data:image/png;base64,iVBORw0KGgo", "detail": "high"},
                    ]}),
                ),
            ],
        )
        .await;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].tool_name.0.as_deref(), Some("view_image"));
        assert_eq!(items[0].text, picture.to_string_lossy());
        assert_eq!(items[1].images.as_deref(), Some([picture.to_string_lossy().into_owned()].as_slice()));
        assert!(!serde_json::to_string(&items).expect("json").contains("iVBORw0KGgo"), "never the bytes");
    }

    #[tokio::test]
    async fn a_plan_in_a_cell_is_a_checklist() {
        let items = parse(&[
            cell(
                "call_A",
                "const plan = [\n  { step: \"Check the tunnel\", status: \"completed\" },\n  { step: \"Move the origin\", status: \"in_progress\" },\n  { step: \"Verify\", status: \"pending\" }\n];\ntext(await tools.update_plan({ plan }));",
            ),
            cell_output("call_A", "Script completed\nWall time 0.0 seconds\nOutput:\n", &["{}"]),
        ])
        .await;
        assert_eq!(items[0].tool_name.0.as_deref(), Some("update_plan"));
        assert_eq!(items[0].text, "1/3 · Move the origin");
        assert_eq!(
            items[0].detail.0.as_deref(),
            Some("[x] Check the tunnel\n[~] Move the origin\n[ ] Verify")
        );
    }

    // ── direct calls ──────────────────────────────────────────────────────

    fn function_call(call_id: &str, name: &str, args: Value) -> String {
        line(
            "response_item",
            json!({"type": "function_call", "name": name, "arguments": args.to_string(), "call_id": call_id}),
        )
    }

    fn function_output(call_id: &str, output: Value) -> String {
        line(
            "response_item",
            json!({"type": "function_call_output", "call_id": call_id, "output": output}),
        )
    }

    /// `exec_command` called directly, the way a client without code mode
    /// gets it — and the `CommandExecution` codex writes for the same call,
    /// under the call's own id, is not a second row.
    #[tokio::test]
    async fn a_direct_command_is_one_row_with_its_header_read_off() {
        let items = parse(&[
            function_call("call_T", "exec_command", json!({"cmd": "raft message check", "workdir": "/w", "yield_time_ms": 10000})),
            completed(command("call_T", "raft message check", 0, "ok\n", json!([{"type": "unknown", "cmd": "raft message check"}]))),
            function_output(
                "call_T",
                json!("Chunk ID: 433404\nWall time: 0.4848 seconds\nProcess exited with code 0\nOriginal token count: 27\nOutput:\n[target=#all] hello\nNo more new inbox messages.\n"),
            ),
        ])
        .await;
        assert_eq!(
            shape(&items),
            vec![
                (ItemKind::ToolUse, Some("shell"), "raft message check"),
                (ItemKind::ToolResult, Some("shell"), "[target=#all] hello"),
            ]
        );
        assert_eq!(items[1].meta.0.as_deref(), Some("exit 0 · 0.5s"));
        assert_eq!(items[1].detail.0.as_deref(), Some("[target=#all] hello\nNo more new inbox messages."));
        assert_eq!(items[1].result_of.as_deref(), Some("call_T"));
    }

    /// A freeform `apply_patch` is a diff too, and the `FileChange` codex wrote
    /// for it supplies the line numbers the envelope does not have.
    #[tokio::test]
    async fn a_direct_patch_is_an_edit_with_the_line_numbers_codex_recorded() {
        let patch = "*** Begin Patch\n*** Update File: MEMORY.md\n@@\n ## Active Context\n-- First startup.\n+- Replied in #all.\n*** End Patch\n";
        let items = parse(&[
            line(
                "response_item",
                json!({"type": "custom_tool_call", "status": "completed", "call_id": "call_P", "name": "apply_patch", "input": patch}),
            ),
            completed(json!({
                "type": "FileChange", "id": "call_P",
                "changes": {"/w/MEMORY.md": {"type": "update", "move_path": null,
                    "unified_diff": "@@ -9,2 +9,2 @@\n ## Active Context\n-- First startup.\n+- Replied in #all.\n"}},
                "status": "completed", "stdout": "", "stderr": "",
            })),
            line(
                "response_item",
                json!({"type": "custom_tool_call_output", "call_id": "call_P",
                       "output": "Exit code: 0\nWall time: 0 seconds\nOutput:\nSuccess. Updated the following files:\nM MEMORY.md\n"}),
            ),
        ])
        .await;
        assert_eq!(
            shape(&items),
            vec![
                (ItemKind::ToolUse, Some("Edit"), "MEMORY.md"),
                (ItemKind::ToolResult, Some("Edit"), "updated MEMORY.md")
            ]
        );
        assert_eq!(
            items[0].detail.0.as_deref(),
            Some("- ## Active Context\n- - First startup.\n+ ## Active Context\n+ - Replied in #all.")
        );
        let hunks = items[1].structured_patch.as_ref().expect("the echo's hunks");
        assert_eq!(hunks[0].old_start, 9);
    }

    #[tokio::test]
    async fn a_failed_patch_says_so_on_every_file() {
        let patch = "*** Begin Patch\n*** Update File: a.rs\n@@\n-x\n+y\n*** Delete File: b.rs\n*** End Patch";
        let items = parse(&[
            line("response_item", json!({"type": "custom_tool_call", "call_id": "call_P", "name": "apply_patch", "input": patch})),
            line(
                "response_item",
                json!({"type": "custom_tool_call_output", "call_id": "call_P",
                       "output": "Exit code: 1\nWall time: 0 seconds\nOutput:\napply_patch verification failed: Failed to find expected lines in a.rs\n"}),
            ),
        ])
        .await;
        assert_eq!(
            shape(&items)[1],
            (ItemKind::ToolUse, Some("Delete"), "b.rs"),
            "every file's row, then every file's result"
        );
        for result in [&items[2], &items[3]] {
            assert!(result.is_error);
            assert_eq!(result.text, "apply_patch verification failed: Failed to find expected lines in a.rs");
        }
        assert_eq!(items[3].result_of.as_deref(), Some("call_P#2"));
    }

    #[tokio::test]
    async fn a_plan_update_is_a_checklist() {
        let items = parse(&[
            function_call(
                "call_U",
                "update_plan",
                json!({"explanation": "two left", "plan": [{"step": "Read", "status": "completed"}, {"step": "Write", "status": "pending"}]}),
            ),
            function_output("call_U", json!("Plan updated")),
        ])
        .await;
        assert_eq!(
            (items[0].text.as_str(), items[0].meta.0.as_deref(), items[0].detail.0.as_deref()),
            ("1/2 steps", Some("two left"), Some("[x] Read\n[ ] Write"))
        );
        assert_eq!(items[1].text, "Plan updated");
    }

    /// The collaboration tools are mostly routing and a fernet blob; who the
    /// call went to is the readable part.
    #[tokio::test]
    async fn the_agent_tools_say_who_and_never_print_the_blob() {
        let items = parse(&[
            line(
                "response_item",
                json!({"type": "function_call", "name": "spawn_agent", "namespace": "collaboration", "call_id": "call_S",
                       "arguments": json!({"task_name": "verify_split", "fork_turns": "all", "message": "gAAAAABqxIduhYoshRd1"}).to_string()}),
            ),
            function_output("call_S", json!("{\"task_name\":\"/root/verify_split\"}")),
            function_call(
                "call_F",
                "followup_task",
                json!({"target": "verify_split", "message": "gAAAAABqxIextgoP8UXi"}),
            ),
            function_output("call_F", json!("")),
            function_call(
                "call_Q",
                "request_user_input_async",
                json!({"questions": [{"title": "Which rate?", "options": ["Today's", "20 yen"]}]}),
            ),
            function_output("call_Q", json!("{\"accepted\":true}")),
        ])
        .await;
        assert_eq!(
            shape(&items),
            vec![
                (ItemKind::ToolUse, Some("spawn_agent"), "verify_split"),
                (ItemKind::ToolResult, Some("spawn_agent"), "{\"task_name\":\"/root/verify_split\"}"),
                (ItemKind::ToolUse, Some("followup_task"), "→ verify_split"),
                (ItemKind::ToolResult, Some("followup_task"), "no output"),
                (ItemKind::ToolUse, Some("request_user_input_async"), "Which rate?"),
                (ItemKind::ToolResult, Some("request_user_input_async"), "{\"accepted\":true}"),
            ]
        );
        assert!(!serde_json::to_string(&items).expect("json").contains("gAAAAA"), "the blob is redacted");
        assert_eq!(items[4].detail.0.as_deref(), Some("Which rate?\n◦ Today's\n◦ 20 yen"));
    }

    #[tokio::test]
    async fn a_local_shell_call_and_the_old_shell_envelope_read_the_same() {
        let items = parse(&[
            line(
                "response_item",
                json!({"type": "local_shell_call", "call_id": "call_L", "status": "completed",
                       "action": {"type": "exec", "command": ["bash", "-lc", "cargo test"], "working_directory": "/w"}}),
            ),
            function_output(
                "call_L",
                json!("{\"output\":\"1 failed\",\"metadata\":{\"exit_code\":101,\"duration_seconds\":3.2}}"),
            ),
            function_call("call_S", "shell", json!({"command": ["git", "commit", "-m", "fix: it"], "workdir": "/w"})),
        ])
        .await;
        assert_eq!(
            shape(&items),
            vec![
                (ItemKind::ToolUse, Some("shell"), "cargo test"),
                (ItemKind::ToolResult, Some("shell"), "1 failed"),
                (ItemKind::ToolUse, Some("shell"), "git commit -m 'fix: it'"),
            ]
        );
        assert_eq!(items[1].meta.0.as_deref(), Some("exit 101 · 3.2s"));
        assert!(items[1].is_error);
    }

    #[tokio::test]
    async fn a_hosted_web_search_is_paired_with_its_own_status() {
        let items = parse(&[line(
            "response_item",
            json!({"type": "web_search_call", "id": "ws_1", "status": "completed", "action": {"type": "search", "query": "codex rollout"}}),
        )])
        .await;
        assert_eq!(
            shape(&items),
            vec![
                (ItemKind::ToolUse, Some("WebSearch"), "codex rollout"),
                (ItemKind::ToolResult, Some("WebSearch"), "completed")
            ]
        );
        assert_eq!(items[1].result_of.as_deref(), Some("ws_1"));
    }

    // ── the pieces ────────────────────────────────────────────────────────

    #[test]
    fn a_lone_call_in_a_cell_is_read_out_of_its_javascript_source() {
        let display = display_codex_custom_tool_call("exec", "await tools.exec_command({cmd: \"ls -a\\ncd /tmp\", workdir: '/w'});");
        assert_eq!(display.tool_name.as_deref(), Some("shell"));
        assert_eq!(display.text, "ls -a ⋯", "only the first line, with an ellipsis for the rest");
        assert_eq!(display.meta.as_deref(), Some("/w"), "a single-quoted JS literal parses too");

        let poll = display_codex_custom_tool_call("exec", "text(await tools.write_stdin({session_id:34720,chars:\"\",yield_time_ms:1000}))");
        assert_eq!(
            (poll.tool_name.as_deref(), poll.text.as_str()),
            (Some("write_stdin"), "poll session 34720")
        );
    }

    #[test]
    fn a_brace_inside_a_command_does_not_end_the_object() {
        let display = display_codex_custom_tool_call("exec", r#"tools.exec_command({cmd: "echo }", workdir: "/w"})"#);
        assert_eq!(display.text, "echo }");
        assert_eq!(display.meta.as_deref(), Some("/w"));
    }

    #[test]
    fn an_apply_patch_envelope_without_files_lists_what_it_can() {
        let patch = "*** Begin Patch\n*** Update File: src/a.rs\n*** Add File: src/b.rs\n*** End Patch";
        let display = display_codex_custom_tool_call("apply_patch", patch);
        assert_eq!(display.tool_name.as_deref(), Some("apply_patch"));
        assert_eq!(display.text, "update src/a.rs, add src/b.rs");
    }

    #[test]
    fn an_envelope_parses_into_files() {
        let files = parse_envelope(
            "*** Begin Patch\n*** Update File: a.rs\n*** Move to: b.rs\n@@ fn main\n ctx\n-old\n+new\n@@\n+tail\n*** End of File\n*** Add File: c.rs\n+one\n+two\n*** Delete File: d.rs\n*** End Patch",
        );
        assert_eq!(files.len(), 3);
        assert_eq!(
            files[0].op,
            FileOp::Update {
                move_to: Some("b.rs".to_string()),
                edits: vec![("ctx\nold".to_string(), "ctx\nnew".to_string()), (String::new(), "tail".to_string())],
                hunks: None,
            }
        );
        assert_eq!(
            files[1].op,
            FileOp::Add {
                content: "one\ntwo".to_string()
            }
        );
        assert_eq!(files[2].op, FileOp::Delete { content: None });
        assert_eq!(file_call(&files[0]).text, "a.rs → b.rs");
        assert_eq!(file_call(&files[0]).tool_name.as_deref(), Some("MultiEdit"));
    }

    #[test]
    fn a_function_call_falls_back_to_the_preferred_key() {
        let display = display_codex_function_call("exec_command", None, r#"{"cmd":"pwd","workdir":"/w"}"#);
        assert_eq!(display.text, "pwd");
        assert_eq!(display.meta.as_deref(), Some("/w"));

        let other = display_codex_function_call("update_plan", None, r#"{"plan":"do the thing","task_name":"t"}"#);
        assert_eq!(other.text, "do the thing", "a plan that is not a list of steps is shown as it is");
        assert_eq!(other.meta.as_deref(), Some("t"));

        let mcp = display_codex_function_call("recall", Some("mcp__banana_memory__"), r#"{"query":"q"}"#);
        assert_eq!((mcp.tool_name.as_deref(), mcp.text.as_str()), (Some("banana_memory:recall"), "q"));

        let unparsed = display_codex_function_call("weird", None, "not json");
        assert_eq!(unparsed.text, "not json", "unparseable arguments still show something");
    }

    #[test]
    fn an_output_envelope_lifts_the_exit_code() {
        let output = json!(r#"{"chunk_id":1,"exit_code":2,"output":"boom\r\nsecond"}"#);
        let display = display_codex_tool_output(Some(&output));
        assert_eq!(display.text, "boom");
        assert_eq!(display.detail.as_deref(), Some("boom\nsecond"), "CRLF from the shell is normalised");
        assert_eq!(display.meta.as_deref(), Some("exit 2"));
        assert!(display.is_error, "a non-zero exit is a failure");
    }

    #[test]
    fn a_script_header_becomes_the_wall_time() {
        let output = json!("Script failed\nWall time 1.5 seconds\nOutput:\nnope");
        let display = display_codex_tool_output(Some(&output));
        assert_eq!(display.text, "nope");
        assert_eq!(display.meta.as_deref(), Some("1.5s"));
        assert!(display.is_error);

        let running = display_codex_tool_output(Some(&json!("Script running with cell ID 8\nWall time 1.0 seconds\nOutput:\n")));
        assert_eq!(running.meta.as_deref(), Some("running · cell 8 · 1.0s"));
        assert_eq!(
            running.text, "Command running in background (cell 8)",
            "the receipt the web draws as running in the background, never as `completed`"
        );
    }

    #[test]
    fn a_truncated_cell_unwraps_each_envelope_line() {
        let output = json!([
            {"type": "input_text", "text": "Script completed\nWall time 0.1 seconds\nOutput:\n"},
            {"type": "input_text", "text": "Warning: truncated output (original token count: 10846)\nTotal output lines: 2\n\n{\"index\":0,\"result\":{\"chunk_id\":\"a\",\"exit_code\":0,\"output\":\"one\\n\"}}\n{\"index\":1,\"result\":{\"chunk_id\":\"b\",\"exit_code\":0,\"output\":\"two\\n\"}}"},
        ]);
        let display = display_codex_tool_output(Some(&output));
        assert_eq!(
            display.detail.as_deref(),
            Some("Warning: truncated output (original token count: 10846)\nTotal output lines: 2\n\none\ntwo")
        );
        assert_eq!(display.meta.as_deref(), Some("0.1s"), "two commands have no one exit code");
    }

    #[test]
    fn output_parts_join_line_by_line() {
        let output = json!([{"text": "a"}, {"text": "b"}]);
        assert_eq!(display_codex_tool_output(Some(&output)).detail.as_deref(), Some("a\nb"));
        assert_eq!(display_codex_tool_output(None).text, "no output");
        let sleep = display_codex_tool_output(Some(&json!("Wall time: 45.0058 seconds\nSleep completed.")));
        assert_eq!((sleep.text.as_str(), sleep.meta.as_deref()), ("Sleep completed.", Some("45.0s")));
    }

    #[test]
    fn explored_commands_read_as_what_they_did() {
        let parsed = [
            json!({"type": "read", "cmd": "sed -n 1,20p a.rs", "name": "a.rs", "path": "src/a.rs"}),
            json!({"type": "read", "cmd": "cat b.rs", "name": "b.rs", "path": "b.rs"}),
            json!({"type": "search", "cmd": "rg -n foo src", "query": "foo", "path": "src"}),
        ];
        assert_eq!(explored(&parsed).as_deref(), Some("Read a.rs, b.rs · Search foo in src"));
        assert_eq!(
            explored(&[json!({"type": "unknown", "cmd": "make"})]),
            None,
            "a guess is worse than the command"
        );
    }

    #[test]
    fn argv_reads_as_the_line_a_shell_would_run() {
        assert_eq!(command_text(Some(&json!(["/bin/zsh", "-lc", "ls | head"]))), "ls | head");
        assert_eq!(command_text(Some(&json!(["git", "log", "--format=%h %s"]))), "git log '--format=%h %s'");
        assert_eq!(
            file_path_of("file:///Users/me/api%E5%B7%A5%E5%85%B7/a%20b.png"),
            "/Users/me/api工具/a b.png"
        );
        assert_eq!(file_path_of("/plain/path"), "/plain/path");
    }
}
