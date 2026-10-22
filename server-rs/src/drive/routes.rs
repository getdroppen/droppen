//! The session-driving routes: start, send, interrupt, rename, archive, browse
//! history, and the composer preferences.
//!
//! Ports of the matching handlers in the former TypeScript server's `app.ts`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use gotcha::{api, GotchaContext, GotchaRouter, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use super::archive;
use super::composer_prefs::{ComposerChoice, ComposerPrefs};
use super::idempotent::{self, Begun, Key};
use crate::api::error::ApiError;
use crate::api::extract::{ApiJson, ApiPath, ApiQuery};
use crate::api::{AppState, SendMessageResult, SendResult};
use crate::auth::MaybeIdentity;
use crate::config::AppConfig;
use crate::feed;
use crate::files;
use crate::inject::questions::{self, Answer, QuestionState, SessionQuestion};
use crate::inject::receipts::{Kind, ReceiptLookup, LOOKUP_CAP};
use crate::inject::{self, AccountChoice, AnswerOutcome, Reply, SendOptions};
use crate::openapi_ext::{double_option, ExtraResponses, Nullable, Optional};
use crate::sessions::checklist::{self, Checklist};
use crate::sessions::util::TranscriptItem;
use crate::sessions::{chats_cwd, claude, codex, file_signature, sendability, transcript, SessionScope, Source};

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
    pub account_from: Optional<inject::AccountFrom>,
    /// A UUID the client minted for this first message, sent unchanged with
    /// every attempt. A repeat answers the session the first attempt started
    /// (`duplicate: true`) instead of starting a second one.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub client_message_id: Optional<String>,
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
    /// The transcript signature the page was read at.
    pub sig: String,
    /// The items in the window.
    pub items: Vec<TranscriptItem>,
    /// Absolute index of the first item.
    pub start_index: usize,
    /// Absolute index one past the last item.
    pub end_index: usize,
    /// Total item count of the transcript.
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

/// Path of `GET /api/sessions/{source}/{id}/agents/{agentId}`.
///
/// A struct rather than a tuple because gotcha only documents one- and
/// two-segment tuple paths (`gotcha_core-0.4.1/src/parameter.rs:43,60`); the
/// generic `Path<T>` impl reads a struct's fields instead, which is the only
/// way to document three.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AgentPath {
    /// Which CLI the parent session belongs to.
    pub source: Source,
    /// The parent session's id.
    pub id: String,
    /// The subagent's id.
    pub agent_id: String,
}

/// Path of the two queue routes.
///
/// A struct for the same reason [`AgentPath`] is one: gotcha documents one- and
/// two-segment tuple paths, and the generic `Path<T>` impl is the only way to
/// document three.
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
/// A struct for the same reason [`QueuePath`] is one: gotcha documents one- and
/// two-segment tuple paths, and the generic `Path<T>` impl is the only way to
/// document three.
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
/// A struct for the same reason [`QueuePath`] is one: gotcha documents one- and
/// two-segment tuple paths, and the generic `Path<T>` impl is the only way to
/// document three.
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
    fn joined(&self) -> String {
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

    fn generate_schema() -> gotcha_core::EnhancedSchema {
        let mut schema = gotcha::oas::Schema {
            _type: None,
            format: None,
            nullable: None,
            description: Some("One label or free text, or every label chosen for a multi-select question.".to_string()),
            extras: Default::default(),
        };
        schema.extras.insert(
            "oneOf".to_string(),
            serde_json::json!([{ "type": "string" }, { "type": "array", "items": { "type": "string" } }]),
        );
        gotcha_core::EnhancedSchema { schema, required: true }
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
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
pub struct FirstUnreadQuery {
    /// The read marker to count from: the `previousReadAt` the reader's
    /// arrival report answered with. Omitted for a session never read, which
    /// counts from the start.
    pub after: Option<String>,
}

// ------------------------------------------------------------------ validation

/// The bounds `SendMessageSchema` and `NewSessionSchema` put on the flag values.
///
/// They are not cosmetic: the values become CLI arguments, so an unbounded
/// `model` could read as a flag rather than as a model name.
pub(crate) fn validate_flags(model: Option<&str>, effort: Option<&str>, permission: Option<&str>) -> Result<(), String> {
    let checks: [(&str, Option<&str>, &'static str, usize); 3] = [
        ("model", model, crate::drive::composer_prefs::MODEL_PATTERN, 100),
        ("effort", effort, r"^[a-z]+$", 20),
        ("permission", permission, r"^[A-Za-z][A-Za-z-]*$", 40),
    ];
    for (field, value, pattern, max_length) in checks {
        let Some(value) = value else { continue };
        let re = regex::Regex::new(pattern).expect("valid flag pattern");
        if value.chars().count() > max_length || !re.is_match(value) {
            return Err(format!("invalid {field}"));
        }
    }
    Ok(())
}

/// `z.string().min(1).max(100_000)`, the bound on every prompt.
pub(crate) fn validate_text(text: &str) -> Result<(), String> {
    match text.chars().count() {
        0 => Err("text is empty".to_string()),
        n if n > 100_000 => Err("text is too long".to_string()),
        _ => Ok(()),
    }
}

/// Every bound the request schema puts on a send body, as one `400`.
fn validate_body(
    text: &str,
    model: Option<&str>,
    effort: Option<&str>,
    permission: Option<&str>,
    account_id: &Option<Option<String>>,
) -> Result<(), ApiError> {
    validate_text(text).map_err(ApiError::bad_request)?;
    validate_flags(model, effort, permission).map_err(ApiError::bad_request)?;
    // `accountId: z.string().uuid().nullable().optional()` — a malformed id is
    // a bad request; a well-formed one that names nothing is a 409 further down.
    if let Some(Some(id)) = account_id {
        if !crate::accounts::store::is_uuid(id) {
            return Err(ApiError::bad_request(format!("invalid account id: {id}")));
        }
    }
    Ok(())
}

fn send_options(model: Option<String>, effort: Option<String>, permission: Option<String>, account_id: Option<Option<String>>) -> SendOptions {
    SendOptions {
        model,
        effort,
        permission,
        account: AccountChoice::from_request(account_id),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------- routes

/// Start a new session in the given directory.
#[api(id = "new_session", group = "sessions", summary = "Start a new session")]
pub async fn new_session(State(state): State<AppState>, ApiJson(body): ApiJson<NewSession>) -> Result<Json<NewSessionResult>, ApiError> {
    validate_body(
        &body.text,
        body.model.as_deref(),
        body.effort.as_deref(),
        body.permission.as_deref(),
        &body.account_id,
    )?;
    let Some(client_id) = body.client_message_id.0.clone() else {
        return start_session(state, body, None).await.map(Json);
    };
    idempotent::validate_id(&client_id)?;
    let receipts = state.drive.injections.receipts().clone();
    let key = Key {
        id: &client_id,
        kind: Kind::NewSession,
        source: body.source,
        session: None,
    };
    let answer = match idempotent::begin(&receipts, key, |_| false).await? {
        Begun::Replay(answer) => idempotent::duplicate(answer),
        Begun::Owned { qid } => {
            let work = async move {
                let started = start_session(state, body, Some(qid)).await?;
                let id = started.id.clone();
                Ok((to_answer(&started)?, Some(id)))
            };
            idempotent::settle(receipts, client_id, work).await?
        }
    };
    from_answer(answer).map(Json)
}

/// Everything [`new_session`] does once its body is valid and, for a named
/// message, its id is claimed.
async fn start_session(state: AppState, body: NewSession, qid: Option<String>) -> Result<NewSessionResult, ApiError> {
    let drive = &state.drive;
    // The first message's attachments pass the gate a later message's do
    // ([`deliver_message`]): every one proven to come from `/api/uploads`, and
    // the resolved path — what was checked — is what reaches the CLI.
    let (images, attachments) = inject::resolve_attachments(&drive.paths, body.images.0.unwrap_or_default(), body.files.0.unwrap_or_default())?;
    let resolved = files::resolve_safe(&drive.paths, Some(&body.cwd)).map_err(|error| error.into_api("invalid working directory"))?;
    // Catch a bad path here: a spawn in a missing cwd only fails asynchronously.
    if !tokio::fs::metadata(&resolved).await.map(|m| m.is_dir()).unwrap_or(false) {
        return Err(ApiError::bad_request(format!("not a directory: {}", resolved.display())));
    }

    let options = SendOptions {
        // Without an explicit pick, new sessions follow the default account
        // when one is configured, and stay local when none is — unless the
        // local login is known to be signed out (`accounts::rotation::start_for`).
        use_default_account: true,
        images,
        files: attachments,
        message_id: qid,
        account: AccountChoice::from_start_request(body.account_id.clone(), body.account_from.0),
        ..send_options(body.model.0, body.effort.0, body.permission.0, body.account_id)
    };
    let id = inject::start_new_session(drive, body.source, &resolved.to_string_lossy(), &body.text, &options)
        .await
        .map_err(|error| spawn_refusal(error, "cannot start session"))?;
    // The sidebar has to show the new session before its first transcript line
    // lands, so the refresh names it and the feed retries until it appears.
    state.global.request_refresh(Some((body.source, &id)));
    Ok(NewSessionResult {
        id,
        duplicate: Optional(None),
    })
}

/// A route's answer as the JSON a retry will be given.
fn to_answer(answer: &impl Serialize) -> Result<serde_json::Value, ApiError> {
    serde_json::to_value(answer).map_err(|error| ApiError::internal(format!("cannot record the answer: {error}")))
}

/// A recorded answer, back in the route's own shape.
fn from_answer<T: serde::de::DeserializeOwned>(answer: serde_json::Value) -> Result<T, ApiError> {
    serde_json::from_value(answer).map_err(|error| ApiError::internal(format!("cannot read the recorded answer: {error}")))
}

/// Hand a message to the local CLI that owns this session.
///
/// With a `clientMessageId` the send is idempotent: the id is claimed before
/// anything is enqueued, a repeat replays the first answer with `duplicate:
/// true`, and a refusal is not recorded. Without one it is the send it always
/// was.
#[api(id = "send_message", group = "sessions", summary = "Send a message")]
pub async fn send_message(
    State(state): State<AppState>,
    #[api(skip)] MaybeIdentity(identity): MaybeIdentity,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiJson(body): ApiJson<SendMessage>,
) -> Result<Json<SendMessageResult>, ApiError> {
    let by = identity.map(|identity| identity.username);
    // Everything zod checks before the TypeScript handler runs is a 400 there
    // (`@hono/zod-validator` answers `c.json(result, 400)`), so it is one here.
    // Before the claim, because none of it depends on anything that changes.
    validate_body(
        &body.text,
        body.model.as_deref(),
        body.effort.as_deref(),
        body.permission.as_deref(),
        &body.account_id,
    )?;
    let Some(client_id) = body.client_message_id.0.clone() else {
        return deliver_message(state, source, id, body, None, by).await.map(Json);
    };
    idempotent::validate_id(&client_id)?;
    let receipts = state.drive.injections.receipts().clone();
    let key = Key {
        id: &client_id,
        kind: Kind::Message,
        source,
        session: Some(&id),
    };
    let injections = state.drive.injections.clone();
    let begun = idempotent::begin(&receipts, key, |qid| injections.holds_queued(source, &id, qid)).await?;
    let answer = match begun {
        Begun::Replay(answer) => idempotent::duplicate(answer),
        Begun::Owned { qid } => {
            let work = {
                let (state, id) = (state.clone(), id.clone());
                async move { Ok((to_answer(&deliver_message(state, source, id, body, Some(qid), by).await?)?, None)) }
            };
            let answer = idempotent::settle(receipts, client_id, work).await?;
            // The receipt the answer just wrote rides on the next state frame.
            injections.poke_feed(source, &id);
            answer
        }
    };
    from_answer(answer).map(Json)
}

/// Everything [`send_message`] does once its body is valid and, for a named
/// message, its id is claimed: `qid` is the queue id that claim chose, and
/// `by` who is sending it.
async fn deliver_message(
    state: AppState,
    source: Source,
    id: String,
    body: SendMessage,
    qid: Option<String>,
    by: Option<String>,
) -> Result<SendMessageResult, ApiError> {
    let drive = &state.drive;
    let images = body.images.0.unwrap_or_default();
    let attachments = body.files.0.unwrap_or_default();
    // Every attachment is proven to come from `/api/uploads` and replaced by its
    // resolved path, so what reaches the CLI is what was checked.
    let (images, attachments) = inject::resolve_attachments(&drive.paths, images, attachments)?;

    let summary = writable_session(drive, source, &id).await?;
    // A session with a resident behind it is never refused for being busy: the
    // message goes into that process' queue and is delivered when the turn in
    // flight ends. See [`sendability`] for why the live-process arm is lifted
    // too — the live process the probe can see is ours.
    let (can_send, reason) = sendability(
        source,
        summary.scope,
        summary.status,
        drive.injections.is_running(source, &id),
        drive.injections.has_resident(source, &id),
    );
    if !can_send {
        return Err(ApiError::conflict(reason.unwrap_or_else(|| "cannot send".to_string())));
    }

    // claude instant chats run with every tool disabled, so attachments can't be read.
    let chat = summary.cwd.as_ref().map(Path::new) == Some(chats_cwd(&drive.paths).as_path());
    if chat && source == Source::Claude && !(images.is_empty() && attachments.is_empty()) {
        return Err(ApiError::conflict("attachments are not supported in claude instant chats"));
    }

    // An agent blocked on a question is waiting for exactly this: what the
    // user says next. Queued, the message would never run — the turn in front
    // of it is the one waiting for the answer — so it goes in as the answer
    // instead, in the user's own words. Anything that stops that (another
    // screen answered a moment ago, the turn was stopped, the session is still
    // reconnecting after a restart) leaves it an ordinary message, delivered
    // the ordinary way below — and the answer says so (`answered: false`), so
    // a client that announced "this goes in as your answer" can take it back.
    //
    // Taken as an answer, the message carries only its words and attachments:
    // `model`, `effort`, `permission` and `accountId` are for a process, and
    // the process is the one blocked mid-turn, which cannot change them. They
    // apply to the next message that starts a turn, as they always have.
    let mut meant_as_answer = false;
    if source == Source::Claude {
        if let Some(request_id) = drive.injections.open_question(&id) {
            meant_as_answer = true;
            let answer = Answer {
                response: Some(inject::claude_prompt(&body.text, &images, &attachments)),
                ..Answer::default()
            };
            if let AnswerOutcome::Delivered(_) = drive
                .injections
                .answer_question(&id, &request_id, Reply::Answer(answer), by.clone())
                .await
            {
                if let Some(qid) = &qid {
                    drive.injections.note_receipt(source, &id, qid, inject::receipts::ReceiptStatus::Sent);
                }
                return Ok(SendMessageResult {
                    accepted: true,
                    queued: false,
                    qid: Optional(qid),
                    duplicate: Optional(None),
                    answered: Optional(Some(true)),
                });
            }
        }
    }

    let options = SendOptions {
        chat,
        images,
        files: attachments,
        message_id: qid.clone(),
        ..send_options(body.model.0, body.effort.0, body.permission.0, body.account_id)
    };
    let sent = inject::send_to_session(drive, source, &id, summary.cwd.as_ref().map(String::as_str), &body.text, &options)
        .await
        .map_err(|error| spawn_refusal(error, "failed to start the CLI"))?;
    Ok(SendMessageResult {
        accepted: true,
        queued: sent.queued,
        qid: Optional(qid),
        duplicate: Optional(None),
        answered: Optional(meant_as_answer.then_some(false)),
    })
}

// -------------------------------------------------------------------- questions

/// The questions the agent stopped to ask in this session, and what became of
/// them. The same list the session's `state` frames carry.
#[api(id = "session_questions", group = "sessions", summary = "Questions the agent asked")]
pub async fn session_questions(
    State(state): State<AppState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
) -> Result<Json<SessionQuestions>, ApiError> {
    let questions = match source {
        Source::Claude => state.drive.injections.questions().for_session(&id, questions::now_ms()),
        Source::Codex => Vec::new(),
    };
    Ok(Json(SessionQuestions { questions }))
}

/// Answer a question the agent is waiting on.
///
/// Into the turn that asked when its process is still holding it, which is the
/// whole point: the agent carries on from the question with the answer in
/// hand. When that process is gone the question has *expired*, and the answer
/// is sent to the session as an ordinary message that says what it answers.
/// Either way, once: a second answer — from another screen, or a double tap —
/// is a `409` carrying the question as the first one left it.
#[api(id = "answer_question", group = "sessions", summary = "Answer a question the agent asked")]
pub async fn answer_question(
    State(state): State<AppState>,
    #[api(skip)] MaybeIdentity(identity): MaybeIdentity,
    ApiPath(QuestionPath { source, id, request_id }): ApiPath<QuestionPath>,
    ApiJson(body): ApiJson<AnswerQuestion>,
) -> Result<Json<QuestionAnswerResult>, ApiError> {
    let answer = Answer {
        answers: body
            .answers
            .iter()
            .map(|(question, value)| (question.clone(), value.joined()))
            .filter(|(_, value)| !value.is_empty())
            .collect(),
        notes: body
            .notes
            .0
            .unwrap_or_default()
            .into_iter()
            .map(|(question, note)| (question, note.trim().to_string()))
            .filter(|(_, note)| !note.is_empty())
            .collect::<BTreeMap<_, _>>(),
        response: body
            .response
            .0
            .map(|response| response.trim().to_string())
            .filter(|response| !response.is_empty()),
    };
    settle_question(
        &state.drive,
        source,
        &id,
        &request_id,
        Reply::Answer(answer),
        identity.map(|identity| identity.username),
    )
    .await
    .map(Json)
}

/// Decline to answer: the agent is told to use its own judgement and carries
/// on. An expired question is simply closed — there is nobody left to tell.
#[api(id = "dismiss_question", group = "sessions", summary = "Dismiss a question the agent asked")]
pub async fn dismiss_question(
    State(state): State<AppState>,
    #[api(skip)] MaybeIdentity(identity): MaybeIdentity,
    ApiPath(QuestionPath { source, id, request_id }): ApiPath<QuestionPath>,
) -> Result<Json<QuestionAnswerResult>, ApiError> {
    settle_question(
        &state.drive,
        source,
        &id,
        &request_id,
        Reply::Dismiss,
        identity.map(|identity| identity.username),
    )
    .await
    .map(Json)
}

/// The `409` a question somebody else already settled is refused with: the
/// question as they left it, so the screen that lost can say who and when.
fn already_settled(question: SessionQuestion) -> ApiError {
    let hint = match (question.state, question.answered_by.0.as_deref()) {
        (QuestionState::Answered, Some(by)) => format!("{by} already answered it"),
        (QuestionState::Answered, None) => "it was already answered".to_string(),
        (QuestionState::Dismissed, _) => "it was already dismissed".to_string(),
        (QuestionState::Cancelled, _) => "the turn that asked it was stopped".to_string(),
        (QuestionState::Expired | QuestionState::Pending, _) => "it can no longer be answered".to_string(),
    };
    ApiError::with_body(
        gotcha::axum::http::StatusCode::CONFLICT,
        serde_json::json!({ "error": "question_settled", "hint": hint, "question": question }),
    )
}

/// Answer or dismiss one question, wherever it stands.
async fn settle_question(
    drive: &crate::drive::Drive,
    source: Source,
    id: &str,
    request_id: &str,
    reply: Reply,
    by: Option<String>,
) -> Result<QuestionAnswerResult, ApiError> {
    // Bounded like any prompt (`validate_text`), and before anything is looked
    // up: every word of it reaches the CLI, in the turn or as a message.
    if let Reply::Answer(answer) = &reply {
        questions::check_answer_size(answer).map_err(ApiError::bad_request)?;
    }
    if source != Source::Claude {
        return Err(ApiError::not_found("no such question"));
    }
    let summary = writable_session(drive, source, id).await?;
    let store = drive.injections.questions().clone();
    let stored = store
        .get(request_id)
        .filter(|stored| stored.session_id == id)
        .ok_or_else(|| ApiError::not_found("no such question"))?;
    if let Reply::Answer(answer) = &reply {
        questions::check_answer(&stored.questions, answer).map_err(ApiError::bad_request)?;
    }
    let settled = match stored.state {
        QuestionState::Pending => match drive.injections.answer_question(id, request_id, reply.clone(), by.clone()).await {
            AnswerOutcome::Delivered(question) => Ok(QuestionAnswerResult {
                question,
                delivered: QuestionDelivery::Turn,
            }),
            AnswerOutcome::Resolved(question) => Err(already_settled(question)),
            AnswerOutcome::Invalid(why) => Err(ApiError::bad_request(why)),
            AnswerOutcome::NotReady => Err(ApiError::conflict(
                "the session is still reconnecting after a server restart; try again in a moment",
            )),
            AnswerOutcome::Unreachable => Err(ApiError::conflict(
                "the answer could not be written to the CLI; the question is still open",
            )),
            // On record as pending, and no process is holding it: whatever
            // asked it is gone, or closed it a moment ago. Read again, so a
            // close that just landed is reported rather than overridden.
            AnswerOutcome::Unknown | AnswerOutcome::NoResident => {
                store.close(request_id, QuestionState::Expired, questions::now_ms());
                match store.get(request_id) {
                    Some(current) if current.state == QuestionState::Expired => settle_expired(drive, &summary, &current, reply, by).await,
                    Some(current) => Err(already_settled(current.wire())),
                    None => Err(ApiError::not_found("no such question")),
                }
            }
        },
        QuestionState::Expired => settle_expired(drive, &summary, &stored, reply, by).await,
        _ => Err(already_settled(stored.wire())),
    };
    if settled.is_ok() {
        drive.injections.announce(source, id);
    }
    settled
}

/// Settle a question whose process is gone: dismissed, it is just closed;
/// answered, the answer goes to the session as an ordinary message.
///
/// The record moves first, as it does for a live question, so two screens
/// cannot both send the message; a send that fails puts it back.
async fn settle_expired(
    drive: &crate::drive::Drive,
    summary: &crate::sessions::SessionSummary,
    stored: &questions::Stored,
    reply: Reply,
    by: Option<String>,
) -> Result<QuestionAnswerResult, ApiError> {
    let store = drive.injections.questions();
    let now = questions::now_ms();
    let answer = match reply {
        Reply::Dismiss => {
            return match store.resolve(
                &stored.request_id,
                &[QuestionState::Expired],
                QuestionState::Dismissed,
                None,
                None,
                by.as_deref(),
                now,
            ) {
                Ok(question) => Ok(QuestionAnswerResult {
                    question: question.wire(),
                    delivered: QuestionDelivery::None,
                }),
                Err(Some(current)) => Err(already_settled(current.wire())),
                Err(None) => Err(ApiError::not_found("no such question")),
            };
        }
        Reply::Answer(answer) => answer,
    };
    // The message names the questions it answers, so it is longer than the
    // answer that was bounded on the way in, and is bounded again as what it
    // now is: a prompt.
    let text = questions::as_message(&stored.questions, &answer);
    if validate_text(&text).is_err() {
        return Err(ApiError::bad_request("the answer is too long to send as a message"));
    }
    let (source, id) = (summary.source, summary.id.as_str());
    let (can_send, reason) = sendability(
        source,
        summary.scope,
        summary.status,
        drive.injections.is_running(source, id),
        drive.injections.has_resident(source, id),
    );
    if !can_send {
        return Err(ApiError::conflict(reason.unwrap_or_else(|| "cannot send".to_string())));
    }
    let question = match store.resolve(
        &stored.request_id,
        &[QuestionState::Expired],
        QuestionState::Answered,
        Some(&answer),
        None,
        by.as_deref(),
        now,
    ) {
        Ok(question) => question,
        Err(Some(current)) => return Err(already_settled(current.wire())),
        Err(None) => return Err(ApiError::not_found("no such question")),
    };
    // The composer's own choice for this session, so the message runs where a
    // message typed into the composer would have: the process that comes back
    // for it must not be one the user never picked.
    let choice = drive
        .composer
        .read()
        .await
        .sessions
        .get(&format!("{source}/{id}"))
        .cloned()
        .unwrap_or_default();
    let options = SendOptions {
        model: choice.model.0,
        effort: choice.effort.0,
        permission: choice.permission.0,
        ..SendOptions::default()
    };
    match inject::send_to_session(drive, source, id, summary.cwd.as_ref().map(String::as_str), &text, &options).await {
        Ok(_) => Ok(QuestionAnswerResult {
            question: question.wire(),
            delivered: QuestionDelivery::Message,
        }),
        Err(error) => {
            store.rewind(&stored.request_id, QuestionState::Answered, QuestionState::Expired, questions::now_ms());
            Err(spawn_refusal(error, "failed to start the CLI"))
        }
    }
}

/// What became of the messages a client named, for the ones it never heard
/// back about.
///
/// The question a client with an unanswered send has to ask before it may send
/// again: an id in `receipts` is in there (do not resend it), an id in
/// `unknown` never arrived (resend it, under the same id), and an id in
/// neither is still being accepted.
#[api(id = "session_receipts", group = "sessions", summary = "Message receipts")]
pub async fn session_receipts(
    State(state): State<AppState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiQuery(query): ApiQuery<ReceiptsQuery>,
) -> Result<Json<ReceiptLookup>, ApiError> {
    let ids: Vec<String> = query
        .ids
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect();
    if ids.is_empty() || ids.len() > LOOKUP_CAP {
        return Err(ApiError::bad_request(format!("ids must name between 1 and {LOOKUP_CAP} messages")));
    }
    state
        .drive
        .injections
        .receipts()
        .lookup(source, &id, &ids, crate::inject::receipts::now_ms())
        .map(Json)
        .map_err(|error| ApiError::internal(format!("cannot read message receipts: {error}")))
}

/// A failed spawn at the status the client should read it at.
///
/// `409` for everything the *session* is in the way of — a busy CLI, a local
/// login, another server's transcript — because those are states that change on
/// their own. Choosing the other CLI's account is not one of them: the request
/// named something that could never have worked, which is the same `400` a
/// malformed `accountId` gets two functions up.
fn spawn_refusal(error: inject::InjectError, fallback: &str) -> ApiError {
    let message = error.client_message(fallback);
    match error {
        inject::InjectError::ProviderMismatch { .. } => ApiError::bad_request(message),
        _ => ApiError::conflict(message),
    }
}

/// The `409` every route that would *change* somebody else's session answers.
///
/// One body, one shape, one token, whether the change was a message, a rename or
/// an archive: the client turns `external_session` into the same read-only
/// notice from any of them.
pub(crate) fn external_session(source: Source) -> ApiError {
    ApiError::with_body(
        gotcha::axum::http::StatusCode::CONFLICT,
        serde_json::json!({
            "error": crate::sessions::EXTERNAL_SESSION_ERROR,
            "hint": crate::sessions::external_send_block_reason(source),
        }),
    )
}

/// The row a mutating route decides on, refusing what is not this server's.
///
/// Every route that changes a session goes through here, and reads the *same*
/// row the list and detail routes serve rather than assembling a second one:
/// `scope` and `status` are what these decisions turn on, and a route that built
/// them itself would drift from what the client was shown.
///
/// Ownership is checked on more than the send path because more than the send
/// path escapes this server. Two of these writes are not private at all:
///
/// * `titles.json` and `archived.json` live under the **shared** config
///   directory, so a rename or an archive by one instance changes what every
///   other instance shows — including for a session it owns and this one does
///   not;
/// * `POST …/codex/{id}/archive` shells out to `codex archive`, which mutates
///   the user's own Codex state for a thread this server did not start.
///
/// Reading is deliberately not gated, and neither is marking a session *seen*:
/// browsing somebody else's session is the entire point of `scope=external`, and
/// an unread badge that could not be cleared would make that browsing worse
/// rather than safer.
pub(crate) async fn writable_session(drive: &crate::drive::Drive, source: Source, id: &str) -> Result<crate::sessions::SessionSummary, ApiError> {
    let mut summary = match source {
        Source::Claude => claude::load_claude_summary(&drive.paths, id).await,
        Source::Codex => codex::find_codex_session(&drive.paths, id).await,
    }
    .ok_or_else(|| ApiError::not_found("session not found"))?;
    // The raw loaders only know the transcript. Apply the same driver snapshot
    // as the list and detail paths before a mutating route decides from this
    // row; in particular, a reaped Codex run outranks the stale `task_started`
    // tail an interrupt can leave behind.
    drive.injections.driver_states().apply(std::slice::from_mut(&mut summary));
    if summary.scope == SessionScope::External {
        return Err(external_session(source));
    }
    Ok(summary)
}

/// Kill the CLI run remote-hand started for this session.
#[api(id = "interrupt_session", group = "sessions", summary = "Interrupt a run")]
pub async fn interrupt_session(
    State(state): State<AppState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
) -> Result<Json<SendResult>, ApiError> {
    if !state.drive.injections.interrupt(source, &id) {
        return Err(ApiError::conflict("no running message to interrupt"));
    }
    Ok(Json(SendResult { accepted: true }))
}

/// Ask claude to stop one background task, leaving the turn in flight alone.
///
/// Separate from [`interrupt_session`] because they are separate verbs. Stop
/// ends the turn and leaves every background task running; this ends one task
/// and leaves the turn — and the other tasks — running. Neither substitutes for
/// the other, and the panel draws them in different places for that reason.
///
/// `accepted` means the request went to claude, not that the task is dead. The
/// CLI reports that by publishing its task list again without the task in it,
/// which reaches the client over the session stream it is already reading — so
/// the row disappearing is the confirmation, and a row that stays is a stop that
/// did not take.
#[api(id = "stop_background_task", group = "sessions", summary = "Stop one background task")]
pub async fn stop_background_task(
    State(state): State<AppState>,
    ApiPath(TaskPath { source, id, task_id }): ApiPath<TaskPath>,
) -> Result<Json<SendResult>, ApiError> {
    match state.drive.injections.stop_background_task(source, &id, &task_id) {
        inject::StopTaskOutcome::Asked => Ok(Json(SendResult { accepted: true })),
        // 404 rather than 409: the task is not there to act on, most often
        // because it finished while the button was being pressed.
        inject::StopTaskOutcome::NoSuchTask => Err(ApiError::not_found("no such background task")),
        // Only a session this server is driving has a channel to ask down.
        inject::StopTaskOutcome::NoResident => Err(ApiError::conflict("this session has no live process to ask")),
    }
}

/// What the queue routes answer, out of what the driver did.
///
/// Every refusal is something the person who pressed the button has to be told,
/// and the two statuses say which kind it is: a `404` means the message is not
/// there to act on — usually because the turn in front of it ended while the
/// request was in flight — and a `409` means it is there and cannot be moved.
fn queue_result(outcome: inject::QueueOutcome) -> Result<Json<SendResult>, ApiError> {
    use inject::QueueOutcome;
    match outcome {
        QueueOutcome::Refused { reason } => Err(ApiError::conflict(reason)),
        QueueOutcome::Done => Ok(Json(SendResult { accepted: true })),
        QueueOutcome::NoResident => Err(ApiError::not_found("this server holds no queue for that session")),
        QueueOutcome::Unknown => Err(ApiError::not_found("no such queued message; it may already have started")),
        QueueOutcome::AlreadySteered => Err(ApiError::conflict(
            "that message has already been handed to the CLI and cannot be taken back",
        )),
        // The one refusal a user cannot act on without knowing *which* setting
        // is in the way, so the driver's own comparison is carried through as
        // the hint. `error` keeps its exact sentence: it is what the schema
        // describes and what older clients display.
        QueueOutcome::NeedsRestart { differences } => Err(ApiError::with_body(
            gotcha::axum::http::StatusCode::CONFLICT,
            serde_json::json!({
                "error": "that message asks for different settings, so it needs a new process rather than a place in this one's queue",
                "hint": format!(
                    "it wants {differences}, and a process is only replaced between turns. \
                     The session will restart for it when the turn in flight ends — or press Stop, and it will start sooner."
                ),
            }),
        )),
        QueueOutcome::Unreachable => Err(ApiError::conflict(
            "CLI delivery could not be confirmed; the message is retained in the queue",
        )),
    }
}

/// Take a message out of a session's queue before it is sent.
#[api(id = "drop_queued_message", group = "sessions", summary = "Drop a queued message")]
pub async fn drop_queued_message(
    State(state): State<AppState>,
    ApiPath(QueuePath { source, id, qid }): ApiPath<QueuePath>,
) -> Result<Json<SendResult>, ApiError> {
    queue_result(state.drive.injections.drop_queued_message(source, &id, &qid).await)
}

/// Hand a queued message to claude now, ahead of the turn in flight.
///
/// Not an interrupt: the turn in flight is left alone and the message is
/// written into claude's stdin, where the CLI holds it and runs it as the next
/// turn — before anything else still queued. What it is for is the case a queue
/// cannot serve at all: an agent that has gone the wrong way, and a correction
/// that is worth nothing twenty minutes from now.
#[api(id = "steer_queued_message", group = "sessions", summary = "Steer a queued message")]
pub async fn steer_queued_message(
    State(state): State<AppState>,
    ApiPath(QueuePath { source, id, qid }): ApiPath<QueuePath>,
) -> Result<Json<SendResult>, ApiError> {
    queue_result(state.drive.injections.steer_queued_message(source, &id, &qid).await)
}

/// Desired sidebar pin state; PUT is idempotent.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct SessionPin {
    /// Whether this session should appear above unpinned sessions.
    pub pinned: bool,
}

/// Pin or unpin an owned session in this instance's sidebar.
#[api(id = "pin_session", group = "sessions", summary = "Pin a session")]
pub async fn pin_session(
    State(state): State<AppState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiJson(body): ApiJson<SessionPin>,
) -> Result<Json<SessionPin>, ApiError> {
    writable_session(&state.drive, source, &id).await?;
    crate::sessions::pins::set(&state.paths, source, &id, body.pinned).map_err(|error| ApiError::internal(error.to_string()))?;
    state.global.request_refresh(Some((source, &id)));
    Ok(Json(body))
}

/// Set a session's display title.
#[api(id = "rename_session", group = "sessions", summary = "Rename a session")]
pub async fn rename_session(
    State(state): State<AppState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiJson(body): ApiJson<SessionRename>,
) -> Result<Json<SessionRename>, ApiError> {
    let drive = &state.drive;
    if body.title.chars().count() > 200 {
        return Err(ApiError::bad_request("title is too long"));
    }
    // `titles.json` is shared by every instance, so a rename here is a rename
    // everywhere — including in the sidebar of the server that actually owns
    // this session.
    writable_session(drive, source, &id).await?;
    let trimmed = body.title.trim().to_string();
    drive
        .titles
        .set(source, &id, &trimmed)
        .await
        .map_err(|error| ApiError::not_found(error.to_string()))?;
    state.global.request_refresh(Some((source, &id)));
    Ok(Json(SessionRename { title: trimmed }))
}

/// Hide a session from the list.
#[api(id = "archive_session", group = "sessions", summary = "Archive a session")]
pub async fn archive_session(State(state): State<AppState>, ApiPath((source, id)): ApiPath<(Source, String)>) -> Result<Json<SendResult>, ApiError> {
    let drive = &state.drive;
    // Both arms leave this server: claude's `archived.json` is shared config, and
    // codex's updates the user's own state through its App Server or CLI. Neither is this
    // server's to do for a session it did not make.
    writable_session(drive, source, &id).await?;
    match source {
        Source::Claude => {
            archive::archive_claude_session(&drive.paths, &id)
                .await
                .map_err(|_| ApiError::conflict("archive failed"))?;
        }
        Source::Codex => {
            drive
                .injections
                .archive_codex(&id)
                .await
                .map_err(|error| ApiError::conflict(error.to_string()))?;
        }
    }
    // An archived session leaves the list, so nothing is named to wait for.
    state.global.request_refresh(None);
    Ok(Json(SendResult { accepted: true }))
}

/// The transcript page immediately before an index the client already holds.
#[api(id = "session_history", group = "sessions", summary = "Older transcript")]
pub async fn session_history(
    State(state): State<AppState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiQuery(query): ApiQuery<HistoryQuery>,
) -> Result<Json<SessionHistoryPage>, ApiError> {
    let drive = &state.drive;
    let not_found = || ApiError::not_found("session not found");
    let state_now = feed::load_session_state(drive, source, &id).await.ok_or_else(not_found)?;
    let file_path = PathBuf::from(&state_now.summary.file_path);
    let sig = file_signature(&file_path).await.ok_or_else(not_found)?;
    let transcript = feed::load_transcript(&drive.paths, source, &file_path, &sig).await;
    let before = query.before as usize;
    // Only the prefix the page depends on is checked, so a session that grew
    // since the client's window was cut is no reason to refuse it.
    if let Some(fp) = query.fp.as_deref() {
        if !feed::history_anchor_holds(&transcript, before, fp, query.since.map(|since| since as usize)) {
            return Err(ApiError::conflict("transcript_changed"));
        }
    }
    let page = feed::transcript_page(&transcript.items, before);
    Ok(Json(SessionHistoryPage {
        sig,
        start_fp: transcript.fp_at(page.start_index).unwrap_or_default(),
        items: page.items,
        start_index: page.start_index,
        end_index: page.end_index,
        total_items: page.total_items,
        has_more_before: page.has_more_before,
    }))
}

/// Search one session's whole transcript, not just the window a client holds.
///
/// Read-only like the detail route, so an external session is as searchable as
/// an owned one.
#[api(id = "session_search", group = "sessions", summary = "Search a session's transcript")]
pub async fn session_search(
    State(state): State<AppState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiQuery(query): ApiQuery<SearchQuery>,
) -> Result<Json<feed::search::SessionSearchResult>, ApiError> {
    search_session(&state.drive, source, &id, &query).await.map(Json)
}

/// [`session_search`] without the extractors, for the tests.
pub(crate) async fn search_session(
    drive: &crate::drive::Drive,
    source: Source,
    id: &str,
    query: &SearchQuery,
) -> Result<feed::search::SessionSearchResult, ApiError> {
    // Before anything is read: a refused query costs nothing.
    let terms = feed::search::validate_query(&query.q).map_err(|error| ApiError::bad_request(error.to_string()))?;
    let limit = query
        .limit
        .map_or(feed::search::DEFAULT_LIMIT, |limit| limit as usize)
        .min(feed::search::MAX_LIMIT);
    // The feed's own parse, cached: a search per keystroke never re-reads the
    // file, and its indices are the ones the feed and the history route use.
    let (items, _) = whole_transcript(drive, source, id).await?;
    let text = query.q.clone();
    // A scan of a long transcript is tens of milliseconds of plain CPU work,
    // which does not belong on a runtime thread serving sockets.
    tokio::task::spawn_blocking(move || feed::search::search_items(&items, &text, &terms, limit, feed::search::TOTAL_CAP))
        .await
        .map_err(|error| ApiError::internal(format!("search failed: {error}")))
}

/// Every item of one session's transcript, by the feed's own cached parse, and
/// the file's mtime (ISO-8601).
///
/// Only the file is looked up, not the row: a claude row's live status costs a
/// read of every live registration and a walk back through the transcript's
/// tail, and nothing that reads the items needs it. The scope is deliberately
/// not checked either — reading is never gated.
async fn whole_transcript(drive: &crate::drive::Drive, source: Source, id: &str) -> Result<(std::sync::Arc<Vec<TranscriptItem>>, String), ApiError> {
    let not_found = || ApiError::not_found("session not found");
    let file_path = match source {
        Source::Claude => claude::find_claude_session_file(&drive.paths, id).await,
        // A database row, and a status read of one 64 KiB tail for a recent
        // rollout only: cheap enough not to need a lookup of its own.
        Source::Codex => codex::find_codex_session(&drive.paths, id)
            .await
            .map(|summary| PathBuf::from(summary.file_path)),
    }
    .ok_or_else(not_found)?;
    let sig = file_signature(&file_path).await.ok_or_else(not_found)?;
    let mtime = tokio::fs::metadata(&file_path)
        .await
        .and_then(|metadata| metadata.modified())
        .map(crate::sessions::iso8601)
        .map_err(|_| not_found())?;
    Ok((feed::load_transcript(&drive.paths, source, &file_path, &sig).await.items.clone(), mtime))
}

/// Where the messages a visit has not read yet begin.
///
/// The client asks once per arrival at a session, with the read marker its
/// arrival's seen report replaced (`previousReadAt` on that report's answer) —
/// so the answer is about what was unread *before* the reader got there, and
/// no later report can move it. Read-only and ungated like search, and answered
/// over the same cached parse, so the index is one the history route can page
/// to. See [`crate::attention::first_unread`].
#[api(id = "session_first_unread", group = "sessions", summary = "Where unread messages begin")]
pub async fn session_first_unread(
    State(state): State<AppState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiQuery(query): ApiQuery<FirstUnreadQuery>,
) -> Result<Json<crate::attention::FirstUnread>, ApiError> {
    first_unread_of(&state.drive, source, &id, query.after.as_deref()).await.map(Json)
}

/// [`session_first_unread`] without the extractors, for the tests.
pub(crate) async fn first_unread_of(
    drive: &crate::drive::Drive,
    source: Source,
    id: &str,
    after: Option<&str>,
) -> Result<crate::attention::FirstUnread, ApiError> {
    let (items, mtime) = whole_transcript(drive, source, id).await?;
    Ok(crate::attention::first_unread(&items, after, &mtime))
}

/// Whether an agent id can name a file next to a transcript.
fn is_agent_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

/// Every subagent that ever wrote under a session.
///
/// The fallback entry point for work the transcript cannot point at. The
/// drill-down in the transcript hangs off a `Task` `tool_use` whose id a meta
/// names, and two real shapes never match one: a Skill-spawned agent, whose meta
/// carries no `toolUseId`, and any agent at `spawnDepth` 2 or deeper, whose
/// `toolUseId` names a call in its parent *agent's* transcript. Everything such
/// an agent did — 2.5 MB across eleven agents in `b16232e3`, 116 of them in
/// another session — was unreachable from the UI. This lists them all, anchored
/// or not, so there is always a way in.
#[api(id = "session_agents", group = "sessions", summary = "Subagents of a session")]
pub async fn session_agents(
    State(state): State<AppState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
) -> Result<Json<SessionAgents>, ApiError> {
    let drive = &state.drive;
    if source != Source::Claude {
        return Err(ApiError::not_found("subagents exist only for claude sessions"));
    }
    let Some(file_path) = claude::find_claude_session_file(&drive.paths, &id).await else {
        return Err(ApiError::not_found("session not found"));
    };
    // Which ids the session's own transcript can point at. Parsed through the
    // shared cache, so the common case — a client opening the panel on a session
    // it is already watching — costs nothing.
    let anchors: HashSet<String> = match file_signature(&file_path).await {
        Some(sig) => feed::parse_transcript(&drive.paths, source, &file_path, &sig)
            .await
            .iter()
            .filter_map(|item| item.tool_use_id.clone())
            .collect(),
        None => HashSet::new(),
    };
    let agents = transcript::list_session_agents(&file_path)
        .await
        .into_iter()
        .map(|agent| SessionAgent {
            anchored: agent.tool_use_id.as_deref().is_some_and(|id| anchors.contains(id)),
            agent_id: agent.agent_id,
            agent_type: Nullable(agent.agent_type),
            description: Nullable(agent.description),
            tool_use_id: Nullable(agent.tool_use_id),
            parent_agent_id: Nullable(agent.parent_agent_id),
            spawn_depth: Nullable(agent.spawn_depth),
            model: Nullable(agent.model),
            bytes: agent.bytes,
            updated_at: Nullable(agent.updated_at),
        })
        .collect();
    Ok(Json(SessionAgents { agents }))
}

/// The transcript of a subagent this session's Task call spawned.
#[api(id = "agent_transcript", group = "sessions", summary = "Subagent transcript")]
pub async fn agent_transcript(
    State(state): State<AppState>,
    ApiPath(AgentPath { source, id, agent_id }): ApiPath<AgentPath>,
) -> Result<Json<AgentTranscript>, ApiError> {
    let drive = &state.drive;
    if source != Source::Claude {
        return Err(ApiError::not_found("subagents exist only for claude sessions"));
    }
    if !is_agent_id(&agent_id) {
        return Err(ApiError::bad_request("invalid agent id"));
    }
    let Some(file_path) = claude::find_claude_session_file(&drive.paths, &id).await else {
        return Err(ApiError::not_found("session not found"));
    };
    // Searched rather than composed: an agent the CLI filed under a
    // subdirectory of `subagents/` is still this session's, and answering 404
    // for it is how a whole class of children stayed unreadable.
    let Some(sub_path) = transcript::find_subagent_file(&file_path, &agent_id).await else {
        return Err(ApiError::not_found("subagent not found"));
    };
    // The meta sits beside the transcript, which for a nested agent is not the
    // top-level directory — reading only that one described it as anonymous.
    let meta = match sub_path.parent() {
        Some(dir) => transcript::list_agent_metas(dir).await.into_iter().find(|meta| meta.agent_id == agent_id),
        None => None,
    };

    let items = transcript::parse_claude_transcript(&drive.paths, &sub_path, true)
        .await
        .unwrap_or_default();
    let (kept, omitted) = feed::truncate_items(&items);
    Ok(Json(AgentTranscript {
        agent_type: Nullable(meta.as_ref().and_then(|m| m.agent_type.clone())),
        description: Nullable(meta.and_then(|m| m.description)),
        total_items: items.len(),
        items: kept,
        omitted_items: omitted,
    }))
}

/// The task checklist Claude Code keeps beside a session.
///
/// Claude-only, like the subagent route above: Codex has no equivalent sidecar,
/// so asking for one is a `404` rather than an empty list that would imply the
/// session simply has not planned anything yet.
#[api(id = "session_checklist", group = "sessions", summary = "Session checklist")]
pub async fn session_checklist(State(state): State<AppState>, ApiPath((source, id)): ApiPath<(Source, String)>) -> Result<Json<Checklist>, ApiError> {
    if source != Source::Claude {
        return Err(ApiError::not_found("checklists exist only for claude sessions"));
    }
    let paths = &state.drive.paths;
    let Some(found) = claude::find_claude_session(paths, &id).await else {
        return Err(ApiError::not_found("session not found"));
    };
    // The sidecar lives under the same config root the transcript was found in.
    let Some(dir) = checklist::tasks_dir(paths, found.root, &id) else {
        return Err(ApiError::not_found("session not found"));
    };
    Ok(Json(checklist::read_checklist(&dir).await))
}

/// Query for `GET /api/sessions/recent-cwds`.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
pub struct RecentCwdsQuery {
    /// Restrict to one CLI's sessions.
    pub source: Option<Source>,
}

/// The directories recent sessions were started in.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
pub struct RecentCwds {
    /// Absolute paths, most recently started in first.
    pub cwds: Vec<String>,
}

/// Working directories to offer when starting a new session.
#[api(id = "recent_cwds", group = "sessions", summary = "Recently used directories")]
pub async fn recent_cwds(State(state): State<AppState>, ApiQuery(query): ApiQuery<RecentCwdsQuery>) -> Json<RecentCwds> {
    let bindings = state.drive.accounts.session_account_map().await;
    let cwds = crate::sessions::recent_cwds(&state.drive.paths, &bindings, query.source).await;
    Json(RecentCwds { cwds })
}

/// Composer model/effort/permission choices, shared across devices.
#[api(id = "composer_prefs", group = "prefs", summary = "Composer preferences")]
pub async fn composer_prefs(State(state): State<AppState>) -> Json<ComposerPrefs> {
    Json(state.drive.composer.read().await)
}

/// Update the source default.
#[api(id = "put_composer_default", group = "prefs", summary = "Set a source's composer default")]
pub async fn put_composer_default(
    State(state): State<AppState>,
    ApiPath(source): ApiPath<Source>,
    ApiJson(body): ApiJson<ComposerChoice>,
) -> Result<Json<ComposerPrefs>, ApiError> {
    state
        .drive
        .composer
        .update(source, None, &body)
        .await
        .map(Json)
        .map_err(|error| ApiError::bad_request(error.to_string()))
}

/// Update one session's entry, and the source default with it.
#[api(id = "put_composer_session", group = "prefs", summary = "Set a session's composer choice")]
pub async fn put_composer_session(
    State(state): State<AppState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiJson(body): ApiJson<ComposerChoice>,
) -> Result<Json<ComposerPrefs>, ApiError> {
    if id.chars().count() > 200 {
        return Err(ApiError::bad_request("session id is too long"));
    }
    state
        .drive
        .composer
        .update(source, Some(&id), &body)
        .await
        .map(Json)
        .map_err(|error| ApiError::bad_request(error.to_string()))
}

/// Register the session-driving routes.
pub fn routes(router: GotchaRouter<GotchaContext<AppState, AppConfig>>) -> GotchaRouter<GotchaContext<AppState, AppConfig>> {
    router
        .post("/api/sessions/new", new_session)
        .post("/api/sessions/{source}/{id}/message", send_message)
        .get("/api/sessions/{source}/{id}/receipts", session_receipts)
        .post("/api/sessions/{source}/{id}/interrupt", interrupt_session)
        .post("/api/sessions/{source}/{id}/tasks/{taskId}/stop", stop_background_task)
        .delete("/api/sessions/{source}/{id}/queue/{qid}", drop_queued_message)
        .post("/api/sessions/{source}/{id}/queue/{qid}/steer", steer_queued_message)
        .get("/api/sessions/{source}/{id}/questions", session_questions)
        .post("/api/sessions/{source}/{id}/questions/{requestId}/answer", answer_question)
        .post("/api/sessions/{source}/{id}/questions/{requestId}/dismiss", dismiss_question)
        .put("/api/sessions/{source}/{id}/title", rename_session)
        .put("/api/sessions/{source}/{id}/pin", pin_session)
        .post("/api/sessions/{source}/{id}/archive", archive_session)
        .get("/api/sessions/{source}/{id}/history", session_history)
        .get("/api/sessions/{source}/{id}/search", session_search)
        .get("/api/sessions/{source}/{id}/first-unread", session_first_unread)
        .get("/api/sessions/{source}/{id}/agents", session_agents)
        .get("/api/sessions/{source}/{id}/agents/{agentId}", agent_transcript)
        .get("/api/sessions/{source}/{id}/checklist", session_checklist)
        .get("/api/sessions/recent-cwds", recent_cwds)
        .get("/api/composer-prefs", composer_prefs)
        .put("/api/composer-prefs/{source}", put_composer_default)
        .put("/api/composer-prefs/{source}/{id}", put_composer_session)
}

/// Every status gotcha cannot derive, plus the golden contract's wordings.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe("PUT", "/api/sessions/{source}/{id}/pin", 200, "The stored pin state")
        .error("PUT", "/api/sessions/{source}/{id}/pin", 400, "Invalid pin state or source")
        .error("PUT", "/api/sessions/{source}/{id}/pin", 404, "Session not found")
        .error(
            "PUT",
            "/api/sessions/{source}/{id}/pin",
            409,
            "The session belongs to another server (`external_session`)",
        )
        .error("PUT", "/api/sessions/{source}/{id}/pin", 500, "Could not persist pin state");
    reg.describe("POST", "/api/sessions/new", 200, "New session started in the given directory")
        .error("POST", "/api/sessions/new", 400, "Invalid working directory")
        .error(
            "POST",
            "/api/sessions/new",
            409,
            "The CLI failed to start a new session, an attachment is not from `/api/uploads` or is no longer on this device (`attachment_missing`), or the `clientMessageId` is still being accepted (`in_progress`) or was used elsewhere (`client_message_id_reused`)",
        )
        .describe(
            "POST",
            "/api/sessions/{source}/{id}/message",
            200,
            "Message handed to the local CLI (claude --print / codex exec resume)",
        )
        .error("POST", "/api/sessions/{source}/{id}/message", 404, "Session not found")
        .error(
            "POST",
            "/api/sessions/{source}/{id}/message",
            409,
            "Session cannot accept input right now, belongs to another server (`external_session`), an attachment is not from `/api/uploads` or is no longer on this device (`attachment_missing`), or the `clientMessageId` is still being accepted (`in_progress`) or was used elsewhere (`client_message_id_reused`)",
        )
        .describe(
            "GET",
            "/api/sessions/{source}/{id}/receipts",
            200,
            "What became of the named messages: accepted ones with their status, and the ones this server never saw",
        )
        .error("GET", "/api/sessions/{source}/{id}/receipts", 400, "No ids, or more than 100")
        .describe(
            "POST",
            "/api/sessions/{source}/{id}/interrupt",
            200,
            "The CLI run remote-hand started for this session was killed",
        )
        .error(
            "POST",
            "/api/sessions/{source}/{id}/interrupt",
            409,
            "No remote-hand message is running for this session",
        )
        .describe(
            "DELETE",
            "/api/sessions/{source}/{id}/queue/{qid}",
            200,
            "The queued message was withdrawn before it reached the CLI",
        )
        .error(
            "DELETE",
            "/api/sessions/{source}/{id}/queue/{qid}",
            404,
            "No queue for this session, or no message with that id: it may already have started",
        )
        .error(
            "DELETE",
            "/api/sessions/{source}/{id}/queue/{qid}",
            409,
            "The message has already been steered into the CLI and cannot be taken back",
        )
        .describe(
            "POST",
            "/api/sessions/{source}/{id}/queue/{qid}/steer",
            200,
            "The queued message was written to the CLI ahead of the turn in flight; it runs next",
        )
        .error(
            "POST",
            "/api/sessions/{source}/{id}/queue/{qid}/steer",
            404,
            "No queue for this session, or no message with that id: it may already have started",
        )
        .error(
            "POST",
            "/api/sessions/{source}/{id}/queue/{qid}/steer",
            409,
            "Already steered, or the message needs settings the running process was not started with",
        )
        .describe(
            "GET",
            "/api/sessions/{source}/{id}/questions",
            200,
            "The questions the agent asked, oldest first: the one it is blocked on and the recent ones with what became of them",
        )
        .describe(
            "POST",
            "/api/sessions/{source}/{id}/questions/{requestId}/answer",
            200,
            "Answered: into the waiting turn (`turn`), or — the process that asked is gone — as a new message (`message`)",
        )
        .error(
            "POST",
            "/api/sessions/{source}/{id}/questions/{requestId}/answer",
            400,
            "An answer keyed by something that is not one of the questions, or no answer at all",
        )
        .error("POST", "/api/sessions/{source}/{id}/questions/{requestId}/answer", 404, "No such question in this session")
        .error(
            "POST",
            "/api/sessions/{source}/{id}/questions/{requestId}/answer",
            409,
            "Already answered, dismissed or withdrawn (`question_settled`, with the question as it stands), or the session cannot take it right now",
        )
        .describe(
            "POST",
            "/api/sessions/{source}/{id}/questions/{requestId}/dismiss",
            200,
            "Dismissed: the agent was told to use its own judgement, or an expired question was closed",
        )
        .error("POST", "/api/sessions/{source}/{id}/questions/{requestId}/dismiss", 404, "No such question in this session")
        .error(
            "POST",
            "/api/sessions/{source}/{id}/questions/{requestId}/dismiss",
            409,
            "Already answered, dismissed or withdrawn (`question_settled`, with the question as it stands)",
        )
        .describe(
            "PUT",
            "/api/sessions/{source}/{id}/title",
            200,
            "The effective custom title (empty when reset to the derived one)",
        )
        .error("PUT", "/api/sessions/{source}/{id}/title", 404, "Session not found")
        .error(
            "PUT",
            "/api/sessions/{source}/{id}/title",
            409,
            "The session belongs to another server (`external_session`); titles are shared config",
        )
        .describe(
            "POST",
            "/api/sessions/{source}/{id}/archive",
            200,
            "Session archived (codex via the official CLI, claude via remote-hand's hidden list)",
        )
        .error("POST", "/api/sessions/{source}/{id}/archive", 404, "Session not found")
        .error(
            "POST",
            "/api/sessions/{source}/{id}/archive",
            409,
            "Archive failed, or the session belongs to another server (`external_session`)",
        )
        .describe(
            "GET",
            "/api/sessions/{source}/{id}/history",
            200,
            "The contiguous transcript page immediately before the requested index",
        )
        .error("GET", "/api/sessions/{source}/{id}/history", 404, "Session not found")
        .error(
            "GET",
            "/api/sessions/{source}/{id}/history",
            409,
            "`transcript_changed`: the items before `before` are no longer the ones `fp` fingerprinted; the client's window needs replacing",
        )
        .describe(
            "GET",
            "/api/sessions/{source}/{id}/search",
            200,
            "Every item of the whole transcript holding all the query's terms: the count, and the newest `limit` with snippets",
        )
        .error(
            "GET",
            "/api/sessions/{source}/{id}/search",
            400,
            "Empty or whitespace-only query, longer than 256 characters, or more than 16 terms",
        )
        .error("GET", "/api/sessions/{source}/{id}/search", 404, "Session not found")
        .describe(
            "GET",
            "/api/sessions/{source}/{id}/first-unread",
            200,
            "The index of the first agent message newer than `after`, and how many there are",
        )
        .error("GET", "/api/sessions/{source}/{id}/first-unread", 404, "Session not found")
        .describe(
            "GET",
            "/api/sessions/{source}/{id}/agents",
            200,
            "Every subagent that wrote a transcript under this session, anchored or not",
        )
        .error(
            "GET",
            "/api/sessions/{source}/{id}/agents",
            404,
            "Session not found, or the source has no subagents",
        )
        .describe(
            "GET",
            "/api/sessions/{source}/{id}/agents/{agentId}",
            200,
            "Transcript of a subagent spawned by this session's Task call",
        )
        .error(
            "GET",
            "/api/sessions/{source}/{id}/agents/{agentId}",
            404,
            "Session or subagent not found",
        )
        .describe(
            "GET",
            "/api/sessions/{source}/{id}/checklist",
            200,
            "The task checklist Claude Code keeps beside this session",
        )
        .error(
            "GET",
            "/api/sessions/{source}/{id}/checklist",
            404,
            "Session not found, or the source has no checklists",
        )
        .describe(
            "GET",
            "/api/sessions/recent-cwds",
            200,
            "Working directories recent sessions were started in, newest first",
        )
        .describe(
            "GET",
            "/api/composer-prefs",
            200,
            "Composer model/effort/permission choices, shared across devices",
        )
        .describe_final("PUT", "/api/composer-prefs/{source}", 200, "Updated composer prefs (source default)")
        .describe_final(
            "PUT",
            "/api/composer-prefs/{source}/{id}",
            200,
            "Updated composer prefs (session entry + source default)",
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drive::Drive;
    use std::sync::Arc;

    fn drive() -> (tempfile::TempDir, Arc<Drive>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Arc::new(crate::sessions::Paths::new(dir.path().to_path_buf()));
        let instance = crate::instance::InstanceDirs::new(&paths.data_dir(), crate::instance::DEFAULT_INSTANCE).expect("valid name");
        let auth = crate::auth::Auth::new(&paths, &instance, &crate::config::AuthSettings::default(), false);
        let drive = Drive::new(paths, &instance, auth);
        (dir, drive)
    }

    #[test]
    fn a_flag_value_that_could_read_as_a_flag_is_refused() {
        assert!(validate_flags(Some("claude-opus-4.5"), Some("high"), Some("acceptEdits")).is_ok());
        assert!(validate_flags(Some("--add-dir"), None, None).is_err());
        assert!(validate_flags(None, Some("High"), None).is_err(), "effort is lowercase only");
        assert!(validate_flags(None, None, Some("-x")).is_err());
        assert!(validate_flags(Some(&"m".repeat(200)), None, None).is_err());
    }

    /// `claude --model claude-opus-4-8[1m]` is how the CLI is asked for the
    /// long-context variant of a model that has one, so a picker that offers it
    /// has to be able to send it. The brackets are the whole point, and the one
    /// thing that must not follow from allowing them is a value a shell would
    /// read as anything but a word.
    ///
    /// They used to get through by accident: the old pattern's `(?-u:\w)` was
    /// written inside a character class, where `?-u` is a range rather than a
    /// flag group, so `[`, `]`, `\` and `` ` `` were all in it — and so was an
    /// *unbalanced* bracket. Hence the second half of this test.
    #[test]
    fn a_long_context_variant_tag_is_a_legal_model_name() {
        assert!(validate_flags(Some("claude-opus-4-8[1m]"), None, None).is_ok());
        assert!(validate_flags(Some("opus[1m]"), None, None).is_ok());
        assert!(validate_flags(Some("claude-opus-5"), None, None).is_ok());
        assert!(validate_flags(Some("claude-opus-4.5"), None, None).is_ok(), "a dotted name still passes");
        assert!(
            validate_flags(Some("anthropic.claude-opus-5:0"), None, None).is_ok(),
            "and a prefixed one"
        );

        for hostile in [
            "opus[1m",
            "opus1m]",
            "opus[]",
            "opus[1m][1m]",
            "[1m]",
            "opus[$(id)]",
            "opus[a;b]",
            "opus[*]",
            "opus[1m] --dangerously-skip-permissions",
            "opus[1m]x",
            "opus[`id`]",
            "opus[a|b]",
            "opus[a b]",
            "opus[a\nb]",
            // Everything the accidental `?-u` range used to let through.
            r"opus\bad",
            "opus`id`",
            "opus^x",
            "opus@x",
            "opus(x)",
        ] {
            assert!(validate_flags(Some(hostile), None, None).is_err(), "{hostile} must be refused");
        }
    }

    #[test]
    fn a_prompt_must_be_non_empty_and_bounded() {
        assert!(validate_text("hi").is_ok());
        assert!(validate_text("").is_err());
        assert!(validate_text(&"x".repeat(100_001)).is_err());
    }

    /// The route's own contract, through the shared helper it now delegates to.
    #[test]
    fn a_cwd_can_never_leave_the_home_directory() {
        let (dir, drive) = drive();
        let home = dir.path();
        let resolve = |requested| files::resolve_safe(&drive.paths, Some(requested));
        assert_eq!(resolve("~").expect("home"), home);
        assert_eq!(resolve("~/proj").expect("under home"), home.join("proj"));
        assert_eq!(resolve("").expect("empty is home"), home);
        assert!(resolve("/etc").is_err());
        assert!(resolve("~/../..").is_err(), "a traversal is caught after normalisation");
    }

    /// An upload on disk, as `POST /api/uploads` leaves one: `uploads/<dir>/<name>`.
    fn uploaded(drive: &Drive, name: &str) -> String {
        let path = drive.paths.uploads_dir().join("abc").join(name);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("uploads dir");
        std::fs::write(&path, b"x").expect("upload");
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn only_paths_inside_the_uploads_directory_may_be_attached() {
        let (_dir, drive) = drive();
        let uploads = drive.paths.uploads_dir();
        let images = vec![uploaded(&drive, "a.png")];
        assert_eq!(
            inject::resolve_attachments(&drive.paths, images.clone(), vec![]).expect("an upload").0,
            images
        );
        assert!(inject::resolve_attachments(&drive.paths, vec!["/etc/passwd".to_string()], vec![]).is_err());
        assert!(
            inject::resolve_attachments(
                &drive.paths,
                vec![],
                vec![uploads.join("../../.ssh/id_rsa").to_string_lossy().into_owned()]
            )
            .is_err(),
            "a traversal out of the uploads dir is not an upload"
        );
    }

    /// An upload that is not there any more — swept with the old ones, or a
    /// draft's attachment kept past that — is refused with a reason of its own,
    /// and so is anything at an upload's path that is not a regular file or
    /// that a symlink takes out of the uploads directory.
    #[test]
    fn an_attachment_must_still_be_a_file_in_the_uploads_directory() {
        let (dir, drive) = drive();
        let uploads = drive.paths.uploads_dir();
        let code = |result: Result<(Vec<String>, Vec<String>), ApiError>| {
            let error = result.expect_err("refused");
            (error.status().as_u16(), error.body()["error"].as_str().map(str::to_string))
        };
        let missing = (409, Some(inject::ATTACHMENT_MISSING.to_string()));

        // Before the uploads directory was ever made…
        assert!(!uploads.exists());
        let gone = uploads.join("abc/gone.png").to_string_lossy().into_owned();
        assert_eq!(code(inject::resolve_attachments(&drive.paths, vec![gone.clone()], vec![])), missing);
        // …and after.
        let real = uploaded(&drive, "kept.txt");
        assert_eq!(
            code(inject::resolve_attachments(&drive.paths, vec![], vec![real.clone(), gone])),
            missing,
            "one gone refuses the message"
        );
        assert_eq!(
            code(inject::resolve_attachments(
                &drive.paths,
                vec![],
                vec![uploads.join("abc").to_string_lossy().into_owned()]
            )),
            missing,
            "a directory is not a file"
        );
        let refused = inject::resolve_attachments(
            &drive.paths,
            vec![uploads.join("abc/also-gone.png").to_string_lossy().into_owned()],
            vec![],
        )
        .expect_err("refused");
        assert_eq!(
            refused.body()["hint"],
            "an attachment is no longer on this device; attach it again",
            "{:?}",
            refused.body()
        );

        #[cfg(unix)]
        {
            let secret = dir.path().join("secret.txt");
            std::fs::write(&secret, b"s").expect("secret");
            let link = uploads.join("abc/link.txt");
            std::os::unix::fs::symlink(&secret, &link).expect("symlink");
            let error = inject::resolve_attachments(&drive.paths, vec![], vec![link.to_string_lossy().into_owned()]).expect_err("refused");
            assert_eq!(
                (error.status().as_u16(), error.body()["error"].as_str()),
                (409, Some("attachment paths must come from /api/uploads")),
                "a symlink out of the uploads directory is not an upload"
            );
        }
        assert!(inject::resolve_attachments(&drive.paths, vec![], vec![real]).is_ok());
    }

    /// The message route refuses a missing upload with the same 409, before it
    /// looks for the session at all.
    #[tokio::test]
    async fn a_message_with_an_attachment_that_is_gone_is_refused() {
        let home = tempfile::tempdir().expect("tempdir");
        let state = crate::api::test_support::state(home.path()).await;
        let gone = state.drive.paths.uploads_dir().join("abc/gone.png").to_string_lossy().into_owned();
        let body = |images: Vec<String>| SendMessage {
            text: "look".into(),
            images: Optional(Some(images)),
            files: Optional(None),
            model: Optional(None),
            effort: Optional(None),
            permission: Optional(None),
            account_id: None,
            client_message_id: Optional(None),
        };
        let id = "99999999-9999-4999-8999-999999999999".to_string();
        let error = send_message(
            State(state.clone()),
            MaybeIdentity(None),
            ApiPath((Source::Claude, id.clone())),
            ApiJson(body(vec![gone])),
        )
        .await
        .expect_err("refused");
        assert_eq!(
            (error.status().as_u16(), error.body()["error"].as_str()),
            (409, Some(inject::ATTACHMENT_MISSING))
        );

        // On disk, it passes; the next thing said no is the session nobody made.
        let path = state.drive.paths.uploads_dir().join("abc/here.png");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("uploads dir");
        std::fs::write(&path, b"x").expect("upload");
        let error = send_message(
            State(state),
            MaybeIdentity(None),
            ApiPath((Source::Claude, id)),
            ApiJson(body(vec![path.to_string_lossy().into_owned()])),
        )
        .await
        .expect_err("no such session");
        assert_eq!(error.status().as_u16(), 404);
    }

    /// A `POST /api/sessions/new` body as a client sends it, with attachments.
    fn new_session_body(cwd: &str, images: Option<Vec<String>>, files: Option<Vec<String>>) -> NewSession {
        NewSession {
            source: Source::Claude,
            cwd: cwd.to_string(),
            text: "look at this".to_string(),
            images: Optional(images),
            files: Optional(files),
            model: Optional(None),
            effort: Optional(None),
            permission: Optional(None),
            account_id: None,
            account_from: Optional(None),
            client_message_id: Optional(None),
        }
    }

    /// The first message takes attachments in the message route's own shape,
    /// and a body without them — every older client's — still reads.
    #[test]
    fn a_new_session_takes_the_message_routes_attachment_fields() {
        let older: NewSession = serde_json::from_str(r#"{"source":"claude","cwd":"/w","text":"hi"}"#).expect("parse");
        assert!(older.images.is_none() && older.files.is_none(), "absent stays absent");

        let body = r#"{"source":"codex","cwd":"/w","text":"hi","images":["/u/a.png"],"files":["/u/b.txt"]}"#;
        let new: NewSession = serde_json::from_str(body).expect("parse");
        let message: SendMessage = serde_json::from_str(body).expect("the message route reads the same fields");
        assert_eq!(new.images.0, Some(vec!["/u/a.png".to_string()]));
        assert_eq!(new.files.0, Some(vec!["/u/b.txt".to_string()]));
        assert_eq!((new.images.0, new.files.0), (message.images.0, message.files.0));
    }

    /// The route, end to end up to the spawn: an attachment the uploads
    /// directory did not hand out is refused with the message route's `409`
    /// before anything is started, a traversal out of it included, and one it
    /// did hand out passes the gate — on to the working-directory check, which
    /// stops this test short of starting a CLI.
    #[tokio::test]
    async fn a_new_sessions_attachments_must_come_from_the_uploads_directory() {
        let home = tempfile::tempdir().expect("tempdir");
        let state = crate::api::test_support::state(home.path()).await;
        let uploads = state.drive.paths.uploads_dir();
        let missing_cwd = home.path().join("no-such-project").to_string_lossy().into_owned();
        let refused = |error: ApiError| (error.status().as_u16(), error.body()["error"].as_str().map(str::to_string));
        let not_an_upload = Some("attachment paths must come from /api/uploads".to_string());

        // Not from `/api/uploads` at all.
        let outside = new_session_body(&missing_cwd, Some(vec!["/etc/passwd".into()]), None);
        let error = new_session(State(state.clone()), ApiJson(outside)).await.expect_err("refused");
        assert_eq!(refused(error), (409, not_an_upload.clone()));

        // A traversal that starts inside the uploads directory and leaves it.
        let escape = uploads.join("abc/../../../.ssh/id_rsa").to_string_lossy().into_owned();
        let traversal = new_session_body(&missing_cwd, None, Some(vec![escape]));
        let error = new_session(State(state.clone()), ApiJson(traversal)).await.expect_err("refused");
        assert_eq!(refused(error), (409, not_an_upload.clone()));

        // The uploads directory itself is not an upload.
        let bare = new_session_body(&missing_cwd, Some(vec![uploads.to_string_lossy().into_owned()]), None);
        let error = new_session(State(state.clone()), ApiJson(bare)).await.expect_err("refused");
        assert_eq!(refused(error), (409, not_an_upload));

        // An upload that is no longer on the device: its own reason, the draft kept.
        let gone = uploads.join("abc/gone.png").to_string_lossy().into_owned();
        let missing = new_session_body(&missing_cwd, Some(vec![gone]), None);
        let error = new_session(State(state.clone()), ApiJson(missing)).await.expect_err("refused");
        assert_eq!(refused(error), (409, Some(inject::ATTACHMENT_MISSING.to_string())));

        // Uploaded paths pass, and the next thing said no is the directory.
        std::fs::create_dir_all(uploads.join("abc")).expect("uploads dir");
        std::fs::write(uploads.join("abc/a.png"), b"x").expect("upload");
        std::fs::write(uploads.join("abc/notes.txt"), b"x").expect("upload");
        let image = uploads.join("abc/a.png").to_string_lossy().into_owned();
        let file = uploads.join("abc/notes.txt").to_string_lossy().into_owned();
        let valid = new_session_body(&missing_cwd, Some(vec![image]), Some(vec![file]));
        let error = new_session(State(state.clone()), ApiJson(valid)).await.expect_err("no such directory");
        let (status, said) = refused(error);
        assert_eq!(status, 400);
        assert!(said.unwrap_or_default().starts_with("not a directory"), "past the attachment gate");

        // And a named attempt refused for its attachments claims nothing: the
        // same id with the attachment fixed is a first attempt, not a replay.
        let mut named = new_session_body(&missing_cwd, Some(vec!["/etc/passwd".into()]), None);
        named.client_message_id = Optional(Some("6f1c2a52-3b7e-4d1f-9a0c-2b8f1e4d5c6a".into()));
        let error = new_session(State(state.clone()), ApiJson(named.clone())).await.expect_err("refused");
        assert_eq!(refused(error).0, 409);
        named.images = Optional(None);
        let error = new_session(State(state), ApiJson(named)).await.expect_err("no such directory");
        assert_eq!(refused(error).0, 400, "answered afresh, not with the recorded refusal");
    }

    #[test]
    fn an_agent_id_is_a_plain_file_name() {
        assert!(is_agent_id("agent_1-2"));
        assert!(!is_agent_id("../escape"));
        assert!(!is_agent_id(""));
        assert!(!is_agent_id(&"a".repeat(65)));
    }

    #[test]
    fn an_absent_account_id_is_not_a_null_one() {
        let absent: SendMessage = serde_json::from_str(r#"{"text":"hi"}"#).expect("parse");
        assert_eq!(AccountChoice::from_request(absent.account_id), AccountChoice::Unset);

        let null: SendMessage = serde_json::from_str(r#"{"text":"hi","accountId":null}"#).expect("parse");
        assert_eq!(AccountChoice::from_request(null.account_id), AccountChoice::Local);

        let named: SendMessage = serde_json::from_str(r#"{"text":"hi","accountId":"abc"}"#).expect("parse");
        assert_eq!(AccountChoice::from_request(named.account_id), AccountChoice::Account("abc".into()));

        let new: NewSession = serde_json::from_str(r#"{"source":"claude","cwd":"/w","text":"hi","accountId":null}"#).expect("parse");
        assert_eq!(AccountChoice::from_request(new.account_id), AccountChoice::Local);
    }

    fn search(q: &str) -> SearchQuery {
        SearchQuery {
            q: q.to_string(),
            limit: None,
        }
    }

    /// A claude transcript in the user's own `~/.claude`, which is a session
    /// this server did not start.
    fn external_claude_session(drive: &Drive, id: &str, lines: &[serde_json::Value]) {
        let project = drive.paths.claude_projects().join("-w-repo");
        std::fs::create_dir_all(&project).expect("project");
        let body: String = lines.iter().map(|line| format!("{line}\n")).collect();
        std::fs::write(project.join(format!("{id}.jsonl")), body).expect("transcript");
    }

    /// The first unread is found against the marker the client kept, in the
    /// numbering the history route pages by — and like search, for a session
    /// that is not this server's own.
    #[tokio::test]
    async fn the_first_unread_is_an_index_in_the_feeds_numbering() {
        let _guard = crate::db::index::exclusive().await;
        let (_dir, drive) = drive();
        let id = "88888888-8888-4888-8888-888888888888";
        let entry = |kind: &str, uuid: &str, at: &str, content: serde_json::Value| serde_json::json!({"type":kind,"message":{"role":kind,"content":content},"timestamp":at,"cwd":"/w/repo","sessionId":id,"uuid":uuid});
        external_claude_session(
            &drive,
            id,
            &[
                entry("user", "u1", "2026-10-01T10:00:00.000Z", serde_json::json!("hi")),
                entry(
                    "assistant",
                    "a1",
                    "2026-10-01T10:00:01.000Z",
                    serde_json::json!([{"type":"text","text":"first answer"}]),
                ),
                entry("user", "u2", "2026-10-01T10:01:00.000Z", serde_json::json!("more")),
                entry(
                    "assistant",
                    "t1",
                    "2026-10-01T10:01:01.000Z",
                    serde_json::json!([{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]),
                ),
                entry(
                    "user",
                    "r1",
                    "2026-10-01T10:01:02.000Z",
                    serde_json::json!([{"type":"tool_result","tool_use_id":"toolu_1","content":"a b"}]),
                ),
                entry(
                    "assistant",
                    "a2",
                    "2026-10-01T10:01:03.000Z",
                    serde_json::json!([{"type":"text","text":"second answer"}]),
                ),
                entry(
                    "assistant",
                    "a3",
                    "2026-10-01T10:01:04.000Z",
                    serde_json::json!([{"type":"text","text":"third answer"}]),
                ),
            ],
        );
        let summary = claude::load_claude_summary(&drive.paths, id).await.expect("the session is there");
        let sig = file_signature(std::path::Path::new(&summary.file_path)).await.expect("sig");
        let items = feed::parse_transcript(&drive.paths, Source::Claude, std::path::Path::new(&summary.file_path), &sig).await;

        let found = first_unread_of(&drive, Source::Claude, id, Some("2026-10-01T10:00:30.000Z"))
            .await
            .expect("readable");
        assert_eq!(found.count, 2);
        let index = found.index.0.expect("an unread message");
        assert_eq!(items[index].text, "second answer", "index {index} is the message it names");

        let never = first_unread_of(&drive, Source::Claude, id, None).await.expect("readable");
        assert_eq!(
            (never.index.0.map(|index| items[index].text.as_str()), never.count),
            (Some("first answer"), 3)
        );
        let read = first_unread_of(&drive, Source::Claude, id, Some("2026-10-01T11:00:00.000Z"))
            .await
            .expect("readable");
        assert_eq!((read.index.0, read.count), (None, 0), "nothing newer than the marker");
        assert!(first_unread_of(&drive, Source::Claude, "77777777-7777-4777-8777-777777777777", None)
            .await
            .is_err());
    }

    /// Reading is never gated: somebody else's session is searched exactly like
    /// one of this server's own, in the feed's numbering.
    #[tokio::test]
    async fn an_external_session_is_searchable_in_the_feeds_numbering() {
        let _guard = crate::db::index::exclusive().await;
        let (_dir, drive) = drive();
        let id = "55555555-5555-4555-8555-555555555555";
        let entry = |kind: &str, uuid: &str, content: serde_json::Value| serde_json::json!({"type":kind,"message":{"role":kind,"content":content},"timestamp":"2026-10-01T10:00:00.000Z","cwd":"/w/repo","sessionId":id,"uuid":uuid});
        external_claude_session(
            &drive,
            id,
            &[
                entry("user", "u1", serde_json::json!("why does the Login page flicker?")),
                entry(
                    "assistant",
                    "a1",
                    serde_json::json!([{"type":"text","text":"The login page re-renders on every tick."}]),
                ),
                entry(
                    "assistant",
                    "t1",
                    serde_json::json!([{"type":"tool_use","id":"toolu_1","name":"Grep","input":{"pattern":"useTicker","path":"src/login"}}]),
                ),
                entry(
                    "user",
                    "r1",
                    serde_json::json!([{"type":"tool_result","tool_use_id":"toolu_1","content":"src/login/page.tsx:12: useTicker()"}]),
                ),
            ],
        );
        let summary = claude::load_claude_summary(&drive.paths, id).await.expect("the session is there");
        assert_eq!(summary.scope, SessionScope::External, "not this server's own");

        let found = search_session(&drive, Source::Claude, id, &search("LOGIN page"))
            .await
            .expect("searchable");
        assert_eq!(found.total, 3);
        let indices: Vec<usize> = found.matches.iter().map(|m| m.index).collect();
        assert_eq!(indices, vec![0, 1, 3], "the result's `src/login/page.tsx` holds both terms too");
        assert_eq!(found.query, "LOGIN page");

        // The same numbering the history route pages by.
        let sig = file_signature(std::path::Path::new(&summary.file_path)).await.expect("sig");
        let items = feed::parse_transcript(&drive.paths, Source::Claude, std::path::Path::new(&summary.file_path), &sig).await;
        let ticker = search_session(&drive, Source::Claude, id, &search("useticker"))
            .await
            .expect("searchable");
        for hit in &ticker.matches {
            let item = &items[hit.index];
            assert_eq!(
                (hit.role, hit.kind),
                (item.role, item.kind),
                "index {} is the item it describes",
                hit.index
            );
        }
        let fields: Vec<feed::search::SearchField> = ticker.matches.iter().map(|m| m.field).collect();
        assert_eq!(
            fields,
            vec![feed::search::SearchField::Detail, feed::search::SearchField::Detail],
            "the pattern is in the call's input and in the result's output"
        );
    }

    #[tokio::test]
    async fn a_codex_thread_is_searchable_too() {
        let _guard = crate::db::index::exclusive().await;
        let (_dir, drive) = drive();
        let id = "66666666-6666-4666-8666-666666666666";
        let day = drive.paths.codex_sessions().join("2026").join("10").join("01");
        std::fs::create_dir_all(&day).expect("rollout dir");
        let lines = [
            r#"{"type":"session_meta","payload":{"cwd":"/w","id":"x"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"列出 迁移脚本"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"look for the migrations first"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"ls migrations\"}"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call_output","output":"0001_init.sql\n0002_迁移.sql"}}"#,
        ];
        std::fs::write(day.join(format!("rollout-2026-10-01T10-00-00-{id}.jsonl")), lines.join("\n") + "\n").expect("rollout");

        let found = search_session(&drive, Source::Codex, id, &search("迁移")).await.expect("searchable");
        let shape: Vec<(usize, crate::sessions::util::ItemKind)> = found.matches.iter().map(|m| (m.index, m.kind)).collect();
        assert_eq!(found.total, 2, "{shape:?}");
        assert_eq!(found.matches[0].field, feed::search::SearchField::Text, "the user's own words");
        assert_eq!(found.matches[1].field, feed::search::SearchField::Detail, "the command's output");

        let calls = search_session(&drive, Source::Codex, id, &search("ls MIGRATIONS"))
            .await
            .expect("searchable");
        assert_eq!(calls.total, 1);
        assert_eq!(calls.matches[0].field, feed::search::SearchField::Tool);
        assert_eq!(calls.matches[0].snippet, "shell · ls migrations");
    }

    /// An answer is bounded like any prompt, before anything is looked up —
    /// so a session that does not even exist answers `400`, not `404`.
    #[tokio::test]
    async fn an_answer_too_long_for_a_prompt_is_refused_before_anything_else() {
        let (_dir, drive) = drive();
        let huge = Answer {
            response: Some("x".repeat(questions::MAX_ANSWER_CHARS + 1)),
            ..Answer::default()
        };
        let error = settle_question(&drive, Source::Claude, "no-such-session", "req", Reply::Answer(huge), None)
            .await
            .expect_err("refused");
        assert_eq!(error.status(), gotcha::axum::http::StatusCode::BAD_REQUEST);
        let error = settle_question(&drive, Source::Claude, "no-such-session", "req", Reply::Dismiss, None)
            .await
            .expect_err("missing");
        assert_eq!(
            error.status(),
            gotcha::axum::http::StatusCode::NOT_FOUND,
            "a dismissal has nothing to bound"
        );
    }

    /// An expired question's answer goes out as a message that also names the
    /// question, so an answer inside the bound can make a message outside it.
    /// It is refused as what it would be — a prompt too long — before the
    /// record moves or anything is sent, and the question stays answerable.
    #[tokio::test]
    async fn an_expired_answer_too_long_as_a_message_is_refused_and_stays_answerable() {
        let _guard = crate::db::index::exclusive().await;
        let (_dir, drive) = drive();
        let id = "99999999-9999-4999-8999-999999999999";
        // A session of this server's own: its transcript in the managed root.
        let project = drive.paths.claude_managed_projects().join("-w-repo");
        std::fs::create_dir_all(&project).expect("project");
        let line = serde_json::json!({"type":"user","message":{"role":"user","content":"hi"},"timestamp":"2026-10-01T10:00:00.000Z","cwd":"/w/repo","sessionId":id,"uuid":"u1"});
        std::fs::write(project.join(format!("{id}.jsonl")), format!("{line}\n")).expect("transcript");

        let asked = "q".repeat(60_000);
        let questions_json = serde_json::json!([{ "question": asked, "header": "Long", "options": [{ "label": "a" }], "multiSelect": false }]);
        let store = drive.injections.questions();
        store.open("req-long", id, Some("toolu_long"), &questions_json, questions::now_ms());
        assert!(store.close("req-long", QuestionState::Expired, questions::now_ms()));

        let answer = Answer {
            answers: BTreeMap::from([(asked.clone(), "a".repeat(50_000))]),
            ..Answer::default()
        };
        assert!(questions::check_answer_size(&answer).is_ok(), "the answer itself is within the bound");
        let error = settle_question(&drive, Source::Claude, id, "req-long", Reply::Answer(answer), None)
            .await
            .expect_err("refused");
        assert_eq!(error.status(), gotcha::axum::http::StatusCode::BAD_REQUEST);
        assert!(error.body().to_string().contains("too long to send as a message"), "{}", error.body());
        assert_eq!(
            store.get("req-long").map(|stored| stored.state),
            Some(QuestionState::Expired),
            "nothing moved, so a shorter answer can still be given"
        );
    }

    #[tokio::test]
    async fn an_empty_query_is_refused_before_the_session_is_looked_up() {
        let (_dir, drive) = drive();
        for blank in ["", "   ", "\t\n"] {
            let error = search_session(&drive, Source::Claude, "no-such-session", &search(blank))
                .await
                .expect_err("refused");
            assert_eq!(error.status(), gotcha::axum::http::StatusCode::BAD_REQUEST, "{blank:?}");
        }
        let error = search_session(&drive, Source::Claude, "no-such-session", &search("x"))
            .await
            .expect_err("missing");
        assert_eq!(error.status(), gotcha::axum::http::StatusCode::NOT_FOUND);
    }
}
