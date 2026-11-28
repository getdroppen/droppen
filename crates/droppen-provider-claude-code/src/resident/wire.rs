//! Stream-JSON wire encoding (stdin) and stdout event classification for Claude.

use std::sync::Arc;

use serde_json::Value;

use droppen_base::process_env::Env;
use droppen_provider::questions::{self, PermissionRequest};
use droppen_provider::shim::Output;
use droppen_provider::RunEntry;

use super::{Cmd, TaskProgress, TaskSeen, LOG_LINE_CAP};

/// One prompt, in the shape claude reads it: a single line of stream-json.
///
/// Shared by the pump and by a steer, which is the point: the two write the
/// same bytes at different moments, and a second copy of this would be a second
/// chance to write a message claude parses differently depending on how it got
/// there.
pub(super) fn user_line(text: &str) -> String {
    user_line_with_images(text, &[])
}

/// The same with a chat's images as base64 content blocks, after the text.
///
/// This is how a tool-less chat receives an image: the block carries the bytes
/// the model sees, and no path is named for a tool that does not exist. The
/// CLI writes an `[Image #N]` marker into the text block for each one.
pub(super) fn user_line_with_images(text: &str, images: &[(&'static str, String)]) -> String {
    let mut content = Vec::with_capacity(1 + images.len());
    content.push(ContentBlock::Text { text });
    for (media_type, data) in images {
        content.push(ContentBlock::Image {
            source: ImageSource {
                kind: "base64",
                media_type,
                data,
            },
        });
    }
    StdinLine::User {
        message: UserMessage { content },
    }
    .line()
}

/// A line of stream-json this server writes to claude's stdin, tagged `type`.
/// The answers to claude's own requests are [`questions::control_response`].
#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum StdinLine<'a> {
    /// A prompt.
    User { message: UserMessage<'a> },
    /// A request of the CLI's own protocol.
    ControlRequest { request_id: String, request: ControlRequest<'a> },
}

/// A user message: a text block, then any image blocks.
#[derive(serde::Serialize)]
#[serde(tag = "role", rename = "user")]
pub(super) struct UserMessage<'a> {
    content: Vec<ContentBlock<'a>>,
}

/// One block of a message, tagged `type`.
#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ContentBlock<'a> {
    /// The prompt's words.
    Text {
        /// The text.
        text: &'a str,
    },
    /// A chat's image, as bytes the model sees.
    Image {
        /// Where the bytes come from and of what type.
        source: ImageSource<'a>,
    },
}

/// One image's inline source, in the API's own shape.
#[derive(serde::Serialize)]
pub(super) struct ImageSource<'a> {
    /// `"base64"`.
    #[serde(rename = "type")]
    kind: &'static str,
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`.
    media_type: &'static str,
    /// The standard base64 payload.
    data: &'a str,
}

/// What a `control_request` asks, tagged `subtype`.
#[derive(serde::Serialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub(super) enum ControlRequest<'a> {
    /// End the turn in flight.
    Interrupt,
    /// Stop one background task.
    StopTask { task_id: &'a str },
}

impl StdinLine<'_> {
    /// The line, without its newline.
    pub(super) fn line(&self) -> String {
        // Strings only. An empty line would be a prompt or a stop claude
        // never got.
        serde_json::to_string(self).expect("stdin lines encode")
    }
}

/// A `Running` injection for one turn of `id`.
///
/// Built here rather than in [`super::Driver::open_turn`] because the send path needs
/// the same thing before there is a driver to ask: see
/// [`super::ClaudeCodeRuns::start_resident`].
pub(super) fn running_turn(id: &str) -> Arc<RunEntry> {
    RunEntry::resident_turn(id)
}

/// Which of the three futures woke the driver.
pub(super) enum Wake {
    Output(Option<Output>),
    Command(Option<Cmd>),
    Tick,
}

/// The handful of things the driver acts on, out of everything claude writes.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Event {
    /// `system:init` — the process is listening, under this session id, and
    /// running this model (the resolved id, not whatever alias started it).
    Init { session: Option<String>, model: Option<String> },
    /// The turn is over.
    Result {
        is_error: bool,
        subtype: String,
        detail: Option<String>,
        /// `api_error_status`: the HTTP status behind a failed turn, when the
        /// CLI says — 429 beside "You've hit your session limit".
        api_status: Option<u16>,
    },
    /// A `rate_limit_event` that is a refusal rather than a reading.
    ///
    /// claude writes one before nearly every turn, almost always saying
    /// `status: allowed`; those are [`Event::UsageReading`]. Only one whose
    /// status is anything else — `rejected`, in every recording so far — is
    /// this, so the limit log can record it (see [`super::Driver::note_rate_limit`]).
    /// Its windows are a reading all the same, and are filed as one. It changes
    /// nothing about the turn.
    RateLimit { info: Value },
    /// A `rate_limit_event` that is not a refusal but carries `unifiedWindows`:
    /// a quota reading of the login this process runs on, and nothing else (see
    /// [`super::Driver::note_usage`]). One without windows says nothing and stays
    /// [`Event::Other`].
    UsageReading { info: Value },
    /// `system:background_tasks_changed` — claude's whole live task list.
    BackgroundTasks { tasks: Vec<TaskSeen> },
    /// `system:task_started` — one more task is running.
    TaskStarted { task: TaskSeen },
    /// `system:task_progress` — how a running task is getting on.
    TaskProgress { progress: TaskProgress },
    /// `system:task_notification` — one task has reached a terminal state.
    TaskFinished { id: String },
    /// The session's own thread said something.
    ///
    /// An `assistant` or `user` event that is this conversation's rather than a
    /// background subagent's, forwarded. The only line on the whole stream that
    /// means "the main agent is working right now", and therefore the only one
    /// that can open a turn nobody wrote a prompt for — see
    /// [`super::Driver::on_speech`].
    Speech,
    /// An answer to something we asked over stdin.
    ControlResponse,
    /// `control_request` / `can_use_tool`: claude wants to run a tool and is
    /// asking whether it may. A question for the user when the tool is
    /// `AskUserQuestion`; a permission prompt, refused, otherwise. Only ever
    /// written by a process started with `--permission-prompt-tool stdio`.
    Permission(Box<PermissionRequest>),
    /// Any other `control_request`. Nothing this server registers for exists,
    /// so each is answered with an error rather than left hanging.
    ControlRequest { request_id: Option<String> },
    /// `control_cancel_request`: claude withdrew a request it had made — what
    /// an interrupt does to a question still waiting for its answer.
    ControlCancel { request_id: String },
    /// A `system` line this driver does not act on: `commands_changed`,
    /// `task_updated`, `thinking_tokens`, and whatever the next CLI release
    /// adds beside them. (`thinking_tokens` does move the draft's counter —
    /// see [`droppen_provider::drafts`] — which is a reading, not an act: the turn, the
    /// queue and the replay are exactly as they would be without it.)
    ///
    /// Told apart from [`Event::Other`] because the *replay* has to treat the
    /// two differently, and reading them alike is what produced ghost turns. The
    /// live rule has always been that a `system` line may not open a turn — see
    /// [`super::Driver::on_speech`] — because the CLI publishes them while the session
    /// sits idle: a background task reporting progress, or the command list
    /// changing when a hook edits it. The replay had no such rule, so a single
    /// `system` line written after the last `result` made the whole history read
    /// as ending mid-turn, and the turn that reconstructed could never end.
    Housekeeping,
    /// A line that is JSON but says nothing this driver acts on.
    Other,
    /// A line that is not JSON at all.
    Unparseable,
}

/// Read one stdout line, refusing to be surprised by it.
///
/// The discipline this encodes is the expensive lesson from every other client
/// of this stream: **parse tolerantly, never drop**. The CLI adds types, adds
/// subtypes, and changes the shape of fields between patch releases, so a reader
/// that validated strictly would keep meeting perfectly good sessions it decided
/// were broken. Everything unrecognised is [`Event::Other`] and everything
/// unparseable is [`Event::Unparseable`]; neither is an error, and neither ends
/// a turn. Only the events above are acted on, and each is read with
/// `unwrap_or` defaults rather than a schema.
///
/// # The background-task events, as claude 2.1.231 actually writes them
///
/// Observed rather than documented — the CLI's own docs describe none of this —
/// by running a real `claude -p --input-format stream-json` and asking it for a
/// `run_in_background` Bash command and a `run_in_background` Agent:
///
/// ```text
/// {"type":"system","subtype":"background_tasks_changed",
///  "tasks":[{"task_id":"b9xgm1tdf","task_type":"local_bash",
///            "description":"Background sleep and file write task"}],…}
/// {"type":"system","subtype":"task_started","task_id":"b9xgm1tdf",
///  "tool_use_id":"toolu_01UC…","description":"…","task_type":"local_bash",…}
/// {"type":"system","subtype":"task_progress","task_id":"…","usage":{…},
///  "last_tool_name":"Bash",…}
/// {"type":"system","subtype":"task_updated","task_id":"b9xgm1tdf",
///  "patch":{"status":"completed","end_time":1787983045880},…}
/// {"type":"system","subtype":"task_notification","task_id":"b9xgm1tdf",
///  "status":"completed","output_file":"…/tasks/b9xgm1tdf.output",
///  "summary":"Background command \"…\" completed (exit code 0)",…}
/// ```
///
/// Three things about that shape decided the code above.
///
/// 1. `background_tasks_changed` is a **snapshot, not a delta** — the whole live
///    list, republished on every change, including the empty list when the last
///    task ends. So it needs no history to be read correctly, which is exactly
///    what a cursor-zero replay needs.
/// 2. A background **Agent** emits `task_started` too, with
///    `task_type:"local_agent"` and a `subagent_type`, and a background Bash the
///    *subagent* then starts emits its own with `owned_by_subagent:true`. (2.1.226
///    spawned background agents through a text channel with no `task_started` at
///    all; that is no longer true on 2.1.231, and the
///    subagent-directory signal in [`super::Driver::minding_background`] is what covers
///    the difference either way.)
/// 3. A task finishing **opens a turn of its own**: `task_notification` is
///    followed by a fresh `system:init`, assistant messages and a `result`, with
///    no user message anywhere. That is why a `result` with no turn in flight is
///    a debug line rather than a failure, and why `init` does not mean "a turn
///    started" — see [`super::Driver::replay`]. It is also why [`Event::Speech`]
///    exists: that turn is real work, and the only thing on the stream that
///    says it is running is the main thread's own `assistant` and `user` events
///    — see [`super::Driver::on_speech`].
pub(super) fn classify(line: &str) -> Event {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Event::Unparseable;
    };
    classify_value(&value)
}

/// [`classify`], for a line the caller has already parsed.
pub(super) fn classify_value(value: &Value) -> Event {
    let subtype = value.get("subtype").and_then(Value::as_str);
    match value.get("type").and_then(Value::as_str) {
        Some("system") if subtype == Some("init") => Event::Init {
            // A missing id is not a reason to refuse the init: the process is
            // listening either way, and the id we asked for stays in force.
            session: value.get("session_id").and_then(Value::as_str).map(str::to_owned),
            model: value.get("model").and_then(Value::as_str).map(str::to_owned),
        },
        // The authoritative one. Every change to what claude is running in the
        // background republishes the whole list, so this needs no history to be
        // read correctly — which is what makes it survive a replay.
        Some("system") if subtype == Some("background_tasks_changed") => Event::BackgroundTasks {
            tasks: value
                .get("tasks")
                .and_then(Value::as_array)
                .map(|tasks| tasks.iter().filter_map(TaskSeen::parse).collect())
                .unwrap_or_default(),
        },
        Some("system") if subtype == Some("task_started") => match TaskSeen::parse(value) {
            Some(task) => Event::TaskStarted { task },
            None => Event::Housekeeping,
        },
        // Not a change to the list, so it never opens or closes anything: it
        // only fills in what a task that is already running is doing.
        Some("system") if subtype == Some("task_progress") => match TaskProgress::parse(value) {
            Some(progress) => Event::TaskProgress { progress },
            None => Event::Housekeeping,
        },
        // `task_notification` is the terminal one and `task_updated` is not:
        // the latter carries a `patch` whose `status` may be anything, and
        // reading a status field that is documented nowhere as the end of a
        // task would be guessing. A notification is only ever sent once the
        // task is over.
        Some("system") if subtype == Some("task_notification") => match value.get("task_id").and_then(Value::as_str) {
            Some(id) => Event::TaskFinished { id: id.to_string() },
            None => Event::Housekeeping,
        },
        // The main thread talking, told from a subagent's by one field.
        //
        // A background `Task` runs its own agent and the CLI republishes that
        // agent's whole stream on the parent's stdout, tagged with the
        // `parent_tool_use_id` of the tool use that started it. The session's
        // own messages carry `null`, or nothing at all. Counted off a real
        // session that ran four background agents (`3c002577`, 2026-08-30):
        // 588 forwarded events, every one of them carrying an id, and every
        // main-thread event without one.
        //
        // `system` lines are deliberately not here even though they too carry a
        // null id: `task_progress` and `task_updated` are published for a
        // background task while the session sits idle, and reading those as the
        // main thread working would leave every session that ever ran one
        // looking permanently busy.
        // Every other `system` subtype, named as what it is rather than left in
        // with the unknowns: the CLI writes these while a session is idle as
        // readily as while it is working, so neither the live path nor the
        // replay may read one as the main thread doing something.
        Some("system") => Event::Housekeeping,
        Some("assistant") | Some("user") if value.get("parent_tool_use_id").is_none_or(Value::is_null) => Event::Speech,
        Some("result") => Event::Result {
            is_error: value.get("is_error").and_then(Value::as_bool).unwrap_or(false),
            subtype: value.get("subtype").and_then(Value::as_str).unwrap_or("an error").to_string(),
            detail: value.get("result").and_then(Value::as_str).map(str::to_owned),
            api_status: value
                .get("api_error_status")
                .and_then(Value::as_u64)
                .and_then(|status| u16::try_from(status).ok()),
        },
        // `allowed` and `allowed_warning` are readings, and a line with no
        // status is not a refusal either; either one is worth something only
        // for the windows it carries.
        Some("rate_limit_event")
            if value
                .pointer("/rate_limit_info/status")
                .and_then(Value::as_str)
                .is_some_and(|status| !status.starts_with("allowed")) =>
        {
            Event::RateLimit {
                info: value["rate_limit_info"].clone(),
            }
        }
        Some("rate_limit_event") if value.pointer("/rate_limit_info/unifiedWindows").is_some_and(Value::is_object) => Event::UsageReading {
            info: value["rate_limit_info"].clone(),
        },
        Some("control_response") => Event::ControlResponse,
        // claude asking *us*. The shapes are [`droppen_provider::questions`]'s to read,
        // and the only place they are spelled.
        Some("control_request") => match PermissionRequest::parse(value) {
            Some(request) => Event::Permission(Box::new(request)),
            None => Event::ControlRequest {
                request_id: value.get("request_id").and_then(Value::as_str).map(str::to_owned),
            },
        },
        Some("control_cancel_request") => match value.get("request_id").and_then(Value::as_str) {
            Some(id) => Event::ControlCancel { request_id: id.to_string() },
            None => Event::Other,
        },
        _ => Event::Other,
    }
}

/// The `tool_use` ids a stdout line carries results for, if it is a `user`
/// event at all. Parsed again rather than carried on [`Event::Speech`]: it is
/// only ever asked while a question or a refusal is outstanding, which is a
/// handful of lines in a session's life.
pub(super) fn answered_in(line: &str) -> Vec<String> {
    serde_json::from_str::<Value>(line)
        .map(|value| questions::answered_tool_uses(&value))
        .unwrap_or_default()
}

/// Which Claude root a spawn with this environment runs in.
///
/// Only a spawn against this instance's own root sets `CLAUDE_CONFIG_DIR` —
/// see `owned_claude_env` — and the server's own value never leaks through,
/// because `claude_env` strips every inherited `CLAUDE_*`. So its absence is the
/// user's own `~/.claude`, and its presence is the managed root.
pub(super) fn recipe_root(env: &Env) -> droppen_instance::paths::ClaudeRoot {
    if env.contains_key("CLAUDE_CONFIG_DIR") {
        droppen_instance::paths::ClaudeRoot::Managed
    } else {
        droppen_instance::paths::ClaudeRoot::Local
    }
}

/// The last `count` lines of a blob, for a failure message.
pub(super) fn last_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

/// A line, shortened enough to log.
pub(super) fn clip(line: &str) -> String {
    clip_to(line, LOG_LINE_CAP)
}

/// The first `cap` characters of `text`, with an ellipsis where it was cut.
///
/// Counted in characters rather than bytes: the cheap byte slice would panic on
/// a multi-byte boundary, and a prompt is the most likely place in this server
/// to meet one.
pub(super) fn clip_to(text: &str, cap: usize) -> String {
    if text.chars().count() <= cap {
        return text.to_string();
    }
    text.chars().take(cap).chain(['…']).collect()
}
