//! Request, response, path, and query DTOs for session-driving routes.

use std::collections::HashMap;

use gotcha::Schematic;
use serde::{Deserialize, Serialize};

use droppen_http::openapi_ext::{double_option, Nullable, Optional};
#[cfg(doc)]
use droppen_http::SendResult;
use droppen_provider::questions;
use droppen_sessions::delivery::{SessionPermission, SessionQuestion};
#[cfg(doc)]
use droppen_sessions::sidecars::AgentPath;
use droppen_sessions::util::TranscriptItem;
use droppen_sessions::Source;

/// `POST /api/sessions/{source}/{id}/message` body.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SendMessage {
    /// The prompt.
    pub text: String,
    /// Image paths returned by `POST /api/uploads`.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub images: Optional<Vec<String>>,
    /// Non-image attachment paths returned by `POST /api/uploads`.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub files: Optional<Vec<String>>,
    /// Model id, passed to the CLI as a flag value.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub model: Optional<String>,
    /// Reasoning effort.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub effort: Optional<String>,
    /// `--permission-mode` (claude) or `--sandbox` (codex) value.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub permission: Optional<String>,
    /// Run under a stored Claude account; `null` means the local login.
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub account_id: Option<Option<String>>,
    /// A UUID the client minted when the message was created, sent unchanged
    /// with every attempt. A repeat is answered with the first attempt's answer
    /// (`duplicate: true`) and enqueues nothing; see `GET …/receipts`.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub client_message_id: Optional<String>,
}

/// The answer to a send, which now has one more thing to say than "done".
///
/// A separate type rather than a field on [`SendResult`] because the other
/// routes that answer with one — rename, archive, interrupt — have no queue and
/// never will, and a field that is always `false` for most of its users is a
/// worse contract than a second shape.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Schematic)]
pub struct SendMessageResult {
    /// Always `true`; a refusal is an error status, not a `false`.
    pub accepted: bool,
    /// The message went in behind a turn that is still running, and will be
    /// delivered when that turn ends. Only a resident claude answers `true`.
    pub queued: bool,
    /// The queue id the message travels under (`resident.queuedMessages[].id`),
    /// when the request carried a `clientMessageId`.
    #[serde(default, skip_serializing_if = "droppen_http::openapi_ext::Optional::is_none")]
    pub qid: droppen_http::openapi_ext::Optional<String>,
    /// `true` when this answers a retry: the `clientMessageId` was already
    /// accepted, nothing was enqueued again, and this is what the first
    /// attempt answered.
    #[serde(default, skip_serializing_if = "droppen_http::openapi_ext::Optional::is_none")]
    pub duplicate: droppen_http::openapi_ext::Optional<bool>,
    /// `true` when the agent was blocked on a question and the message went in
    /// as its answer (the question's free-text `response`) rather than into the
    /// queue behind the turn it would otherwise have waited for; `false` when a
    /// question was open but could not take it — answered from another screen a
    /// moment before, stopped, or the session still reconnecting — so it was
    /// delivered as an ordinary message (`queued` says whether it waits).
    /// Absent when no question was open.
    #[serde(default, skip_serializing_if = "droppen_http::openapi_ext::Optional::is_none")]
    pub answered: droppen_http::openapi_ext::Optional<bool>,
}

/// `POST /api/sessions/new` body.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct NewSession {
    /// Which CLI to start.
    pub source: Source,
    /// Working directory for the new session.
    pub cwd: String,
    /// The first prompt.
    pub text: String,
    /// Image paths returned by `POST /api/uploads`, sent with the first
    /// message exactly as `POST /api/sessions/{source}/{id}/message` takes them.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub images: Optional<Vec<String>>,
    /// Non-image attachment paths returned by `POST /api/uploads`, sent with
    /// the first message.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub files: Optional<Vec<String>>,
    /// Model id.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub model: Optional<String>,
    /// Reasoning effort.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub effort: Optional<String>,
    /// Permission / sandbox mode.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub permission: Optional<String>,
    /// Run under a stored Claude account; `null` means the local login.
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub account_id: Option<Option<String>>,
    /// `"start"`: `accountId` is the account `GET /api/accounts/start` named
    /// for a pair no model-less start reaches (`placed: false`), not a pick —
    /// logged as the start rule's choice and checked again at the spawn.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub account_from: Optional<droppen_provider::AccountFrom>,
    /// A UUID the client minted for this first message, sent unchanged with
    /// every attempt. A repeat answers the session the first attempt started
    /// (`duplicate: true`) instead of starting a second one.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub client_message_id: Optional<String>,
    /// The configured ACP agent to start, by id (`GET /api/acp/agents`):
    /// required when `source` is `acp`, refused otherwise.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub acp_agent: Optional<String>,
}

/// The id of the session that was just created.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct NewSessionResult {
    /// The CLI's own session id.
    pub id: String,
    /// `true` when this answers a retry of an already-accepted
    /// `clientMessageId`: no second session was started.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub duplicate: Optional<bool>,
}

/// Query for `GET /api/sessions/{source}/{id}/receipts`.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
pub struct ReceiptsQuery {
    /// Comma-separated `clientMessageId`s, at most 100.
    pub ids: String,
}

/// `PUT /api/sessions/{source}/{id}/title` body and response.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct SessionRename {
    /// New display title; empty resets to the derived title.
    pub title: String,
}

/// A contiguous page of an older part of a transcript.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SessionHistoryPage {
    /// The transcript signature the page was read at: with `fp` in the
    /// query, possibly an older one than the file has now, since a page that
    /// fingerprint proves is the same in every parse holding it.
    pub sig: String,
    /// The items in the window.
    pub items: Vec<TranscriptItem>,
    /// Absolute index of the first item.
    pub start_index: usize,
    /// Absolute index one past the last item.
    pub end_index: usize,
    /// Total item count of the transcript at `sig`.
    pub total_items: usize,
    /// Whether older items exist before `start_index`.
    pub has_more_before: bool,
    /// Fingerprint of every item before `start_index`: the window's new start
    /// fingerprint once this page is prepended, and what the next page down is
    /// asked for with.
    pub start_fp: String,
}

/// A subagent's own transcript.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AgentTranscript {
    /// Which agent type answered.
    pub agent_type: Nullable<String>,
    /// What the Task call asked for.
    pub description: Nullable<String>,
    /// The items, head and tail of an over-long transcript.
    pub items: Vec<TranscriptItem>,
    /// How many items the transcript holds in total.
    pub total_items: usize,
    /// How many were dropped out of the middle.
    pub omitted_items: usize,
}

/// One agent that wrote a transcript under a session, however it was spawned.
///
/// The index behind the drill-down. Every field but the id may be missing,
/// because the meta the CLI writes beside an agent may itself be missing or
/// half-filled — a Skill-spawned agent's meta is `{"agentType":…,"spawnDepth":1}`
/// and nothing else — and an agent this server cannot describe is exactly the
/// one worth listing, since it is the one nothing else in the UI can reach.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SessionAgent {
    /// The agent's id, as `GET …/agents/{agentId}` takes it.
    pub agent_id: String,
    /// Which agent type answered.
    pub agent_type: Nullable<String>,
    /// What the spawning call asked for.
    pub description: Nullable<String>,
    /// The `tool_use` id its meta recorded, if any.
    pub tool_use_id: Nullable<String>,
    /// The agent that spawned it, for anything deeper than the session.
    pub parent_agent_id: Nullable<String>,
    /// How deep the spawn chain is: `1` is a direct child of the session.
    pub spawn_depth: Nullable<u32>,
    /// Which model it ran on.
    pub model: Nullable<String>,
    /// Whether the transcript names a `tool_use` this agent hangs off.
    ///
    /// False for the two shapes that have no home in the main transcript: a
    /// meta with no `toolUseId` at all (Skills spawn these), and a `toolUseId`
    /// that belongs to a *parent agent's* transcript rather than the session's
    /// (anything at `spawnDepth` 2 or deeper). Both are why this list exists:
    /// an unanchored agent is invisible everywhere else, and one recorded
    /// session hid eleven of them and 2.5 MB of work that way.
    pub anchored: bool,
    /// Size of its transcript on disk, in bytes.
    pub bytes: u64,
    /// When it was last written, by the filesystem's clock, ISO-8601.
    pub updated_at: Nullable<String>,
}

/// Every agent found under a session.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SessionAgents {
    /// Most recently written first.
    pub agents: Vec<SessionAgent>,
}

/// Path of the two queue routes.
///
/// A struct for [`AgentPath`]'s reason: a tuple would lose the field docs that
/// are this route's parameter descriptions.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct QueuePath {
    /// Which CLI the session belongs to. Only `claude` has a queue.
    pub source: Source,
    /// The session's id.
    pub id: String,
    /// The queued message's id, from `resident.queuedMessages[].id`.
    pub qid: String,
}

/// Path of the stop-one-background-task route.
///
/// A struct for [`AgentPath`]'s reason: a tuple would lose the field docs that
/// are this route's parameter descriptions.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct TaskPath {
    /// Which CLI the session belongs to. Only `claude` mints background tasks.
    pub source: Source,
    /// The session's id.
    pub id: String,
    /// The task's id, from `resident.tasks[].id` — claude's own `task_id`.
    ///
    /// Not the `toolUseId` beside it: that is the transcript's name for the call
    /// that launched the task, and the CLI's stop request is addressed by this.
    pub task_id: String,
}

/// Path of the two question routes.
///
/// A struct for [`AgentPath`]'s reason: a tuple would lose the field docs that
/// are this route's parameter descriptions.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct QuestionPath {
    /// Which CLI the session belongs to. Only `claude` asks.
    pub source: Source,
    /// The session's id.
    pub id: String,
    /// The question's id, from `questions[].requestId`.
    pub request_id: String,
}

/// One question's answer: a label or the user's own words, or — for a
/// multi-select question — every label chosen.
///
/// Both shapes because both are what a client naturally holds: a picked option
/// is a string, a set of ticked ones is a list. The server does the joining the
/// CLI wants ([`questions::join_choices`]), so no client has to know that
/// claude takes a multi-select answer as one string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnswerValue {
    /// One choice, or free text.
    One(String),
    /// Several choices.
    Many(Vec<String>),
}

impl AnswerValue {
    /// The answer as the CLI takes it.
    pub(super) fn joined(&self) -> String {
        match self {
            AnswerValue::One(text) => text.trim().to_string(),
            AnswerValue::Many(choices) => questions::join_choices(choices),
        }
    }
}

impl Schematic for AnswerValue {
    fn name() -> &'static str {
        "AnswerValue"
    }

    fn required() -> bool {
        true
    }

    fn type_() -> &'static str {
        "string"
    }

    fn generate_schema() -> gotcha::EnhancedSchema {
        let mut schema = gotcha::oas::Schema {
            description: Some("One label or free text, or every label chosen for a multi-select question.".to_string()),
            ..Default::default()
        };
        schema.extras.insert(
            "oneOf".to_string(),
            serde_json::json!([{ "type": "string" }, { "type": "array", "items": { "type": "string" } }]),
        );
        gotcha::EnhancedSchema { schema, required: true }
    }
}

/// `POST /api/sessions/{source}/{id}/questions/{requestId}/answer` body.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AnswerQuestion {
    /// The exact question text, as asked → its answer. May be empty when
    /// `response` says it all.
    #[serde(default)]
    pub answers: HashMap<String, AnswerValue>,
    /// The exact question text → a note beside the choice.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub notes: Optional<HashMap<String, String>>,
    /// Free text, with the choices or instead of them.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub response: Optional<String>,
}

/// Where an answer went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum QuestionDelivery {
    /// Into the turn that asked — the answer, or for a dismissal the refusal:
    /// the agent carries on from the question.
    Turn,
    /// The process that asked is gone, so the answer was sent to the session
    /// as an ordinary message saying what it answers.
    Message,
    /// Nothing was sent: an expired question was dismissed.
    None,
}

/// What answering or dismissing a question did.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct QuestionAnswerResult {
    /// The question as it now stands.
    pub question: SessionQuestion,
    /// Where the answer went.
    pub delivered: QuestionDelivery,
}

/// A session's questions.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SessionQuestions {
    /// Oldest first: the one the agent is blocked on, if any, and the recent
    /// ones with what became of them.
    pub questions: Vec<SessionQuestion>,
}

/// `POST /api/sessions/{source}/{id}/permissions/{requestId}` body.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct RespondPermission {
    /// The option id chosen by the user (`SessionPermission::options[].id`).
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub option_id: Optional<String>,
    /// Set `true` to cancel/decline without selecting a specific option.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub cancel: Optional<bool>,
}

/// What answering a tool permission prompt did.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRespondResult {
    /// The permission prompt as it now stands.
    pub permission: SessionPermission,
}

/// A session's tool permission prompts.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SessionPermissions {
    /// Oldest first: the ones the agent is blocked on, and the recent settled ones.
    pub permissions: Vec<SessionPermission>,
}

/// Query for `GET /api/sessions/{source}/{id}/history`.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
pub struct HistoryQuery {
    /// Exclusive absolute index before which the previous page is loaded.
    pub before: u32,
    /// The client's fingerprint of a prefix of the transcript it holds, proving
    /// the page will meet its window: of items `[0, before)` (the window's
    /// `startFp`), or — when `since` is given — of `[0, since - 3)`, the resume
    /// marker the session feed last sent (`prefixFp`). Absent, the page is
    /// served unchecked and the client compares `sig` itself.
    pub fp: Option<String>,
    /// The `endIndex` of the client's window, when `fp` is its resume marker.
    pub since: Option<u32>,
    /// The absolute index the page should reach back to, for a client that
    /// knows the item it wants — a jump to a search hit or to the first unread
    /// message: the page is then the items `[from, before)` in one answer
    /// instead of the one page before `before`. Never fewer items than that
    /// page, and at most 2000 items and 1 MiB of them; past either cap it is
    /// the newest part of the range, and its `startIndex`, later than `from`
    /// and with `hasMoreBefore`, is where the next request picks up. `fp`
    /// proves it exactly as it proves a page. Absent, one page, as before.
    pub from: Option<u32>,
}

/// Query for `GET /api/sessions/{source}/{id}/search`.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
pub struct SearchQuery {
    /// What to look for. Split on whitespace; an item matches when it holds
    /// every term, case-insensitively, anywhere in its text, its tool call or
    /// its output. At most 256 characters and 16 terms.
    pub q: String,
    /// How many matches to describe, newest first; default 200, at most 1000.
    /// `total` counts every match regardless.
    pub limit: Option<u32>,
}

/// Query for `GET /api/sessions/{source}/{id}/first-unread`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, Schematic)]
pub struct FirstUnreadQuery {
    /// The read marker to count from: the `previousReadAt` the reader's
    /// arrival report answered with. Omitted for a session never read, which
    /// counts from the start.
    pub after: Option<String>,
    /// The `endIndex` of the client's window, when `fp` is its resume marker.
    pub since: Option<u32>,
    /// The resume marker the session feed last sent the client (`prefixFp`):
    /// its fingerprint of items `[0, since - 3)`. With `since`, it lets the
    /// answer come from the newest parse the server already holds instead of a
    /// fresh read of the transcript, when that parse holds the client's whole
    /// window, the first unread message lies inside the part the marker
    /// proves, and the file was last written no more than 10 seconds (and 8
    /// MiB) after that parse. `count` then counts through the end of that
    /// parse, which is at least the client's window; messages written in
    /// those few seconds reach the client through the feed. Without them, or
    /// when the parse cannot be proven, the transcript is read as it is now.
    pub fp: Option<String>,
}
