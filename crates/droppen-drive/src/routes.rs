//! The session-driving routes: start, send, interrupt, rename, archive, browse
//! history, and the composer preferences.
//!
//! Ports of the matching handlers in the former TypeScript server's `app.ts`.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gotcha::axum::http::StatusCode;
use gotcha::axum::extract::FromRef;
use gotcha::{api, GotchaRouter, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use super::attachments::resolve_attachments;
use super::composer_prefs::{ComposerChoice, ComposerPrefs};
use super::idempotent::{self, Begun, Key};
use droppen_accounts::store::{AccountError, AccountProvider};
use droppen_accounts::{account_not_found, provider_mismatch, AccountInputExt};
use droppen_auth::MaybeIdentity;
use crate::feed;
use droppen_http::error::{ApiError, Hinted};
use droppen_http::SendResult;
use droppen_http::extract::{ApiJson, ApiPath, ApiQuery};
use droppen_provider::permissions;
use droppen_provider::questions::{self, Answer};
use droppen_provider::receipts::{Kind, LOOKUP_CAP};
use droppen_provider::{AccountChoice, AnswerOutcome, ContinueError, PermissionOutcome, PermissionReply, Reply, SendOptions};
use droppen_http::openapi_ext::{required_nullable, Nullable, Optional};
use droppen_instance::paths::ClaudeRoot;
use droppen_sessions::delivery::{PermissionState, QuestionState, ReceiptLookup, ReceiptStatus, SessionPermission, SessionQuestion};
use droppen_sessions::sidecars::{is_agent_id, AgentPath, Checklist};
use droppen_sessions::util::TranscriptItem;
use droppen_provider::models::ModelRule;
use droppen_sessions::{chats_cwd, file_signature, SessionScope, Source};


mod accounts;
mod interactive;
mod transcript;
mod types;
#[cfg(test)]
mod tests;

pub use accounts::*;
pub use interactive::*;
pub use transcript::*;
pub use types::*;
#[cfg(test)]
use interactive::{already_settled, expired_answer_options, settle_question};
#[cfg(test)]
use transcript::{FIRST_UNREAD_MAX_LAG_BYTES, FIRST_UNREAD_MAX_LAG_MS};

// ------------------------------------------------------------------ validation

/// The bounds `SendMessageSchema` and `NewSessionSchema` put on the flag values,
/// and the composer's choices and a scheduled task's flags with them: a `model`
/// spelled the way its CLI's `rule` allows (the provider's
/// [`droppen_provider::models::Models::model_rule`]), then the effort and the permission
/// mode.
///
/// They are not cosmetic: the values become CLI arguments, so an unbounded
/// `model` could read as a flag rather than as a model name.
pub(crate) fn validate_flags(rule: ModelRule, model: Option<&str>, effort: Option<&str>, permission: Option<&str>) -> Result<(), String> {
    if model.is_some_and(|model| !rule.admits(model)) {
        return Err("invalid model".to_string());
    }
    static EFFORT_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static PERMISSION_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    if let Some(value) = effort {
        let re = EFFORT_RE.get_or_init(|| regex::Regex::new(r"^[a-z][a-z0-9_-]*$").expect("valid effort pattern"));
        if value.chars().count() > 32 || !re.is_match(value) {
            return Err("invalid effort".to_string());
        }
    }
    if let Some(value) = permission {
        let re = PERMISSION_RE.get_or_init(|| regex::Regex::new(r"^[A-Za-z][A-Za-z-]*$").expect("valid permission pattern"));
        if value.chars().count() > 40 || !re.is_match(value) {
            return Err("invalid permission".to_string());
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
///
/// The model id is held to its CLI's own `rule` (an ACP agent spells its ids
/// its own way), and an ACP agent's to its own list when the message is
/// delivered.
fn validate_body(
    rule: ModelRule,
    text: &str,
    model: Option<&str>,
    effort: Option<&str>,
    permission: Option<&str>,
    account_id: &Option<Option<String>>,
) -> Result<(), ApiError> {
    validate_text(text).map_err(ApiError::bad_request)?;
    validate_flags(rule, model, effort, permission).map_err(ApiError::bad_request)?;
    // `accountId: z.string().uuid().nullable().optional()` — a malformed id is
    // a bad request; a well-formed one that names nothing is a 409 further down.
    if let Some(Some(id)) = account_id {
        if !droppen_accounts::store::is_uuid(id) {
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

/// What the session-driving routes use of the server's state.
#[derive(Clone)]
pub struct DriveState {
    /// Accounts, injections, quota, titles and composer preferences.
    pub drive: Arc<super::Drive>,
    /// Where sessions are discovered.
    pub paths: Arc<droppen_instance::paths::Paths>,
    /// The shared session-list feed, told when a session appears or changes.
    pub global: Arc<feed::global::GlobalFeed>,
    /// The same feed's inputs, which the session-binding routes tell.
    pub refresh: droppen_http::refresh::Refresh,
}
droppen_http::substate!(DriveState);

/// Start a new session in the given directory.
#[api(
    id = "new_session",
    group = "sessions",
    summary = "Start a new session",
    responses(200 = "New session started in the given directory"),
    errors(
        400 = "Invalid working directory",
        409 = "The CLI failed to start a new session, an attachment is not from `/api/uploads` or is no longer on this device (`attachment_missing`), or the `clientMessageId` is still being accepted (`in_progress`) or was used elsewhere (`client_message_id_reused`)"
    )
)]
pub async fn new_session(State(state): State<DriveState>, ApiJson(body): ApiJson<NewSession>) -> Result<Json<NewSessionResult>, ApiError> {
    validate_body(
        state.drive.providers.model_rule(body.source),
        &body.text,
        body.model.as_deref(),
        body.effort.as_deref(),
        body.permission.as_deref(),
        &body.account_id,
    )?;
    state.drive.providers.check_start(body.source, body.acp_agent.as_deref())?;
    let Some(client_id) = body.client_message_id.0.clone() else {
        return start_session(state, body, None).await.map(Json);
    };
    idempotent::validate_id(&client_id)?;
    let receipts = state.drive.injections.host.receipts().clone();
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
async fn start_session(state: DriveState, body: NewSession, qid: Option<String>) -> Result<NewSessionResult, ApiError> {
    let drive = &state.drive;
    // The first message's attachments pass the gate a later message's do
    // ([`deliver_message`]): every one proven to come from `/api/uploads`, and
    // the resolved path — what was checked — is what reaches the CLI.
    let (images, attachments) = resolve_attachments(&drive.paths, body.images.0.unwrap_or_default(), body.files.0.unwrap_or_default())?;
    let resolved = droppen_files::resolve_safe(&drive.paths, Some(&body.cwd)).map_err(|error| error.into_api("invalid working directory"))?;
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
        acp_agent: body.acp_agent.0,
        ..send_options(body.model.0, body.effort.0, body.permission.0, body.account_id)
    };
    let id = drive.providers.start_new_session(body.source, &resolved.to_string_lossy(), &body.text, &options)
        .await
        .map_err(|error| spawn_refusal(error, "cannot start session"))?;
    let sent_uploads: Vec<String> = options.images.iter().chain(options.files.iter()).cloned().collect();
    if !sent_uploads.is_empty() {
        droppen_files::FileRoots::of(&drive.paths).mark_uploads_sent(&sent_uploads).await;
    }
    // The mode this session started under is its own from now on. A pick in
    // another session moves only the source default, which an existing session
    // never reads; recording it here keeps that true even for a client that
    // does not pin it itself (#563). This is the start's explicit choice, so a
    // dangerous mode is stored as such.
    if let Some(permission) = options.permission.as_deref() {
        if let Err(error) = drive
            .composer
            .record_start_permission(body.source, drive.providers.model_rule(body.source), &id, permission)
            .await
        {
            // The process is already running: a preference row that could not
            // be written is not a reason to tell the client the start failed.
            tracing::warn!(session = %id, %error, "cannot record the new session's permission");
        }
    }
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
#[api(
    id = "send_message",
    group = "sessions",
    summary = "Send a message",
    responses(200 = "Message handed to the local CLI (claude --print / codex exec resume)"),
    errors(
        404 = "Session not found",
        409 = "Session cannot accept input right now, belongs to another server (`external_session`), an attachment is not from `/api/uploads` or is no longer on this device (`attachment_missing`), or the `clientMessageId` is still being accepted (`in_progress`) or was used elsewhere (`client_message_id_reused`)"
    )
)]
pub async fn send_message(
    State(state): State<DriveState>,
    #[api(skip)] MaybeIdentity(identity): MaybeIdentity,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiJson(body): ApiJson<SendMessage>,
) -> Result<Json<SendMessageResult>, ApiError> {
    let by = identity.map(|identity| identity.username);
    // Everything zod checks before the TypeScript handler runs is a 400 there
    // (`@hono/zod-validator` answers `c.json(result, 400)`), so it is one here.
    // Before the claim, because none of it depends on anything that changes.
    validate_body(
        state.drive.providers.model_rule(source),
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
    let receipts = state.drive.injections.host.receipts().clone();
    let key = Key {
        id: &client_id,
        kind: Kind::Message,
        source,
        session: Some(&id),
    };
    let injections = state.drive.injections.clone();
    let providers = state.drive.providers.clone();
    let begun = idempotent::begin(&receipts, key, |qid| providers.holds_queued(source, &id, qid)).await?;
    let answer = match begun {
        Begun::Replay(answer) => idempotent::duplicate(answer),
        Begun::Owned { qid } => {
            let work = {
                let (state, id) = (state.clone(), id.clone());
                async move { Ok((to_answer(&deliver_message(state, source, id, body, Some(qid), by).await?)?, None)) }
            };
            let answer = idempotent::settle(receipts, client_id, work).await?;
            // The receipt the answer just wrote rides on the next state frame.
            injections.host.poke_feed(source, &id);
            answer
        }
    };
    from_answer(answer).map(Json)
}

/// Everything [`send_message`] does once its body is valid and, for a named
/// message, its id is claimed: `qid` is the queue id that claim chose, and
/// `by` who is sending it.
async fn deliver_message(
    state: DriveState,
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
    let (images, attachments) = resolve_attachments(&drive.paths, images, attachments)?;

    let summary = writable_session(drive, source, &id).await?;
    // A session with a resident behind it is never refused for being busy: the
    // message goes into that process' queue and is delivered when the turn in
    // flight ends. See [`TranscriptSources::sendability`] for why the
    // live-process arm is lifted too — the live process the probe can see is
    // ours. An ACP session whose agent was removed is refused whatever is
    // running. This is the refusal the client reads; `acp_send` looks the agent
    // up again under the spawn gate, the backstop for one removed after this
    // check.
    let (can_send, reason) = match drive.providers.send_block(&summary) {
        Some(reason) => (false, Some(reason)),
        None => drive.transcripts.sendability(
            source,
            summary.scope,
            summary.status,
            drive.providers.is_running(source, &id),
            drive.providers.has_resident(source, &id),
        ),
    };
    if !can_send {
        return Err(ApiError::conflict(reason.unwrap_or_else(|| "cannot send".to_string())));
    }

    // A CLI whose chats cannot read an attachment (no tool to open one) is
    // refused here. Both CLIs' chats can now: Claude Code runs its chats with
    // Read as its only tool, and Codex takes images as input and non-images by
    // path; the check stays as the guard for any provider added later.
    let chat = summary.cwd.as_ref().map(Path::new) == Some(chats_cwd(&drive.paths).as_path());
    if chat && drive.providers.chat_refuses_attachments(source) && !(images.is_empty() && attachments.is_empty()) {
        return Err(ApiError::conflict("attachments are not supported in this instant chat"));
    }
    // A chat that takes attachments as content (a claude chat, which has no
    // tools) is read now and refused if it cannot be delivered as base64
    // blocks or inlined text.
    if chat && drive.providers.capabilities(source).chat_inline_attachments {
        super::attachments::validate_claude_chat_attachments(&drive.paths, &images, &attachments).await?;
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
    if drive.providers.capabilities(source).questions {
        if let Some(request_id) = drive.providers.open_question(source, &id) {
            meant_as_answer = true;
            let answer = Answer {
                response: Some(droppen_provider::claude_prompt(&body.text, &images, &attachments)),
                ..Answer::default()
            };
            if let AnswerOutcome::Delivered(_) = drive
                .providers
                .answer_question(source, &id, &request_id, Reply::Answer(answer), by.clone())
                .await
            {
                let sent_uploads: Vec<String> = images.iter().chain(attachments.iter()).cloned().collect();
                if !sent_uploads.is_empty() {
                    droppen_files::FileRoots::of(&drive.paths).mark_uploads_sent(&sent_uploads).await;
                }
                if let Some(qid) = &qid {
                    drive.injections.host.note_receipt(source, &id, qid, ReceiptStatus::Sent);
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

    let requested_permission = body.permission.0.clone();
    let mut options = SendOptions {
        chat,
        images,
        files: attachments,
        message_id: qid.clone(),
        ..send_options(body.model.0, body.effort.0, body.permission.0, body.account_id)
    };
    // A chat is pinned server-side: whatever the body says, it is never
    // restarted with a permission mode. The chat marker is the session's own
    // working directory, not anything the client sent.
    if chat {
        options.permission = None;
    }
    let sent = drive.providers.send_to_session(source, &id, summary.cwd.as_ref().map(String::as_str), &body.text, &options)
        .await
        .map_err(|error| spawn_refusal(error, "failed to start the CLI"))?;
    // A human turn clears any consecutive cross-session hop streak on this session.
    drive.agents.reset_hops(source, &id);
    let sent_uploads: Vec<String> = options.images.iter().chain(options.files.iter()).cloned().collect();
    if !sent_uploads.is_empty() {
        droppen_files::FileRoots::of(&drive.paths).mark_uploads_sent(&sent_uploads).await;
    }
    // A session that had no mode of its own gets the one this message ran
    // with — unless that mode is dangerous, which a message may carry from a
    // stale cache or an older build and must never turn into the session's
    // choice. An existing permission (a pick, or the mode the session started
    // with) is never overwritten here; only an explicit pick in the session
    // can change it. The row is written only after the CLI took the message:
    // an answer delivered into a turn in flight (above) changed no process,
    // and a send that failed did not run with anything.
    // A chat has no permission of its own to record: the mode in the body was
    // dropped above, and a row here would only resurface in a later restore.
    if !chat {
        if let Some(permission) = requested_permission.as_deref() {
            if let Err(error) = drive
                .composer
                .pin_permission(source, drive.providers.model_rule(source), &id, permission)
                .await
            {
                tracing::warn!(session = %id, %error, "cannot fill the session's permission");
            }
        }
    }
    Ok(SendMessageResult {
        accepted: true,
        queued: sent.queued,
        qid: Optional(qid),
        duplicate: Optional(None),
        answered: Optional(meant_as_answer.then_some(false)),
    })
}

/// What became of the messages a client named, for the ones it never heard
/// back about.
///
/// The question a client with an unanswered send has to ask before it may send
/// again: an id in `receipts` is in there (do not resend it), an id in
/// `unknown` never arrived (resend it, under the same id), and an id in
/// neither is still being accepted.
#[api(
    id = "session_receipts",
    group = "sessions",
    summary = "Message receipts",
    responses(200 = "What became of the named messages: accepted ones with their status, and the ones this server never saw"),
    errors(400 = "No ids, or more than 100")
)]
pub async fn session_receipts(
    State(state): State<DriveState>,
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
        .host.receipts()
        .lookup(source, &id, &ids, droppen_provider::receipts::now_ms())
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
fn spawn_refusal(error: droppen_provider::InjectError, fallback: &str) -> ApiError {
    let message = error.client_message(fallback);
    match error {
        droppen_provider::InjectError::ProviderMismatch { .. } => ApiError::bad_request(message),
        _ => ApiError::conflict(message),
    }
}

/// The `409` every route that would *change* somebody else's session answers.
///
/// One body, one shape, one token, whether the change was a message, a rename or
/// an archive: the client turns `external_session` into the same read-only
/// notice from any of them.
pub(crate) fn external_session(hint: String) -> ApiError {
    ApiError::refusal(
        gotcha::axum::http::StatusCode::CONFLICT,
        Hinted {
            error: droppen_sessions::EXTERNAL_SESSION_ERROR,
            hint: &hint,
        },
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
pub(crate) async fn writable_session(drive: &crate::Drive, source: Source, id: &str) -> Result<droppen_sessions::SessionSummary, ApiError> {
    let mut summary = drive
        .transcripts
        .find_summary(&drive.paths, source, id)
        .await
        .ok_or_else(|| ApiError::not_found("session not found"))?;
    // The raw loaders only know the transcript. Apply the same driver snapshot
    // as the list and detail paths before a mutating route decides from this
    // row; in particular, a reaped Codex run outranks the stale `task_started`
    // tail an interrupt can leave behind.
    drive.providers.driver_states().apply(&drive.transcripts, std::slice::from_mut(&mut summary));
    if summary.scope == SessionScope::External {
        return Err(external_session(drive.transcripts.external_reason(source)));
    }
    Ok(summary)
}

/// Kill the CLI run remote-hand started for this session.
#[api(
    id = "interrupt_session",
    group = "sessions",
    summary = "Interrupt a run",
    responses(200 = "The CLI run remote-hand started for this session was killed"),
    errors(409 = "No remote-hand message is running for this session")
)]
pub async fn interrupt_session(
    State(state): State<DriveState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
) -> Result<Json<SendResult>, ApiError> {
    if !state.drive.providers.interrupt(source, &id) {
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
    State(state): State<DriveState>,
    ApiPath(TaskPath { source, id, task_id }): ApiPath<TaskPath>,
) -> Result<Json<SendResult>, ApiError> {
    match state.drive.providers.stop_background_task(source, &id, &task_id) {
        droppen_provider::StopTaskOutcome::Asked => Ok(Json(SendResult { accepted: true })),
        // 404 rather than 409: the task is not there to act on, most often
        // because it finished while the button was being pressed.
        droppen_provider::StopTaskOutcome::NoSuchTask => Err(ApiError::not_found("no such background task")),
        // Only a session this server is driving has a channel to ask down.
        droppen_provider::StopTaskOutcome::NoResident => Err(ApiError::conflict("this session has no live process to ask")),
    }
}

/// What the queue routes answer, out of what the driver did.
///
/// Every refusal is something the person who pressed the button has to be told,
/// and the two statuses say which kind it is: a `404` means the message is not
/// there to act on — usually because the turn in front of it ended while the
/// request was in flight — and a `409` means it is there and cannot be moved.
fn queue_result(outcome: droppen_provider::QueueOutcome) -> Result<Json<SendResult>, ApiError> {
    use droppen_provider::QueueOutcome;
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
        QueueOutcome::NeedsRestart { differences } => Err(ApiError::refusal(
            gotcha::axum::http::StatusCode::CONFLICT,
            Hinted {
                error: "that message asks for different settings, so it needs a new process rather than a place in this one's queue",
                hint: &format!(
                    "it wants {differences}, and a process is only replaced between turns. \
                     The session will restart for it when the turn in flight ends — or press Stop, and it will start sooner."
                ),
            },
        )),
        QueueOutcome::Unreachable => Err(ApiError::conflict(
            "CLI delivery could not be confirmed; the message is retained in the queue",
        )),
    }
}

/// Take a message out of a session's queue before it is sent.
#[api(
    id = "drop_queued_message",
    group = "sessions",
    summary = "Drop a queued message",
    responses(200 = "The queued message was withdrawn before it reached the CLI"),
    errors(
        404 = "No queue for this session, or no message with that id: it may already have started",
        409 = "The message has already been steered into the CLI and cannot be taken back"
    )
)]
pub async fn drop_queued_message(
    State(state): State<DriveState>,
    ApiPath(QueuePath { source, id, qid }): ApiPath<QueuePath>,
) -> Result<Json<SendResult>, ApiError> {
    queue_result(state.drive.providers.drop_queued_message(source, &id, &qid).await)
}

/// Hand a queued message to claude now, ahead of the turn in flight.
///
/// Not an interrupt: the turn in flight is left alone and the message is
/// written into claude's stdin, where the CLI holds it and runs it as the next
/// turn — before anything else still queued. What it is for is the case a queue
/// cannot serve at all: an agent that has gone the wrong way, and a correction
/// that is worth nothing twenty minutes from now.
#[api(
    id = "steer_queued_message",
    group = "sessions",
    summary = "Steer a queued message",
    responses(200 = "The queued message was written to the CLI ahead of the turn in flight; it runs next"),
    errors(
        404 = "No queue for this session, or no message with that id: it may already have started",
        409 = "Already steered, or the message needs settings the running process was not started with"
    )
)]
pub async fn steer_queued_message(
    State(state): State<DriveState>,
    ApiPath(QueuePath { source, id, qid }): ApiPath<QueuePath>,
) -> Result<Json<SendResult>, ApiError> {
    queue_result(state.drive.providers.steer_queued_message(source, &id, &qid).await)
}

/// Desired sidebar pin state; PUT is idempotent.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct SessionPin {
    /// Whether this session should appear above unpinned sessions.
    pub pinned: bool,
}

/// Pin or unpin an owned session in this instance's sidebar.
#[api(
    id = "pin_session",
    group = "sessions",
    summary = "Pin a session",
    responses(200 = "The stored pin state"),
    errors(
        400 = "Invalid pin state or source",
        404 = "Session not found",
        409 = "The session belongs to another server (`external_session`)",
        500 = "Could not persist pin state"
    )
)]
pub async fn pin_session(
    State(state): State<DriveState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiJson(body): ApiJson<SessionPin>,
) -> Result<Json<SessionPin>, ApiError> {
    writable_session(&state.drive, source, &id).await?;
    droppen_sessions::pins::set(&state.paths, source, &id, body.pinned).map_err(|error| ApiError::internal(error.to_string()))?;
    state.global.request_refresh(Some((source, &id)));
    Ok(Json(body))
}

/// Set a session's display title.
#[api(
    id = "rename_session",
    group = "sessions",
    summary = "Rename a session",
    responses(200 = "The effective custom title (empty when reset to the derived one)"),
    errors(404 = "Session not found", 409 = "The session belongs to another server (`external_session`); titles are shared config")
)]
pub async fn rename_session(
    State(state): State<DriveState>,
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
#[api(
    id = "archive_session",
    group = "sessions",
    summary = "Archive a session",
    responses(200 = "Session archived (codex via the official CLI, claude via remote-hand's hidden list)"),
    errors(404 = "Session not found", 409 = "Archive failed, or the session belongs to another server (`external_session`)")
)]
pub async fn archive_session(State(state): State<DriveState>, ApiPath((source, id)): ApiPath<(Source, String)>) -> Result<Json<SendResult>, ApiError> {
    let drive = &state.drive;
    // Both arms leave this server: claude's `archived.json` is shared config, and
    // codex's updates the user's own state through its App Server or CLI. Neither is this
    // server's to do for a session it did not make.
    writable_session(drive, source, &id).await?;
    drive.providers.archive(source, &id).await.map_err(ApiError::conflict)?;
    // A chat's delivery rows have nothing left to annotate once it is hidden.
    droppen_sessions::chat_delivery::prune_session(&drive.paths, source, &id);
    // An archived session leaves the list, so nothing is named to wait for.
    state.global.request_refresh(None);
    Ok(Json(SendResult { accepted: true }))
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
#[api(
    id = "recent_cwds",
    group = "sessions",
    summary = "Recently used directories",
    responses(200 = "Working directories recent sessions were started in, newest first")
)]
pub async fn recent_cwds(State(state): State<DriveState>, ApiQuery(query): ApiQuery<RecentCwdsQuery>) -> Json<RecentCwds> {
    let bindings = state.drive.accounts.session_account_map().await;
    let cwds = state.drive.transcripts.recent_cwds(&state.drive.paths, &bindings, query.source).await;
    Json(RecentCwds { cwds })
}

/// The working directories of sessions that are no longer there.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
pub struct GoneCwds {
    /// Absolute paths, sorted.
    pub cwds: Vec<String>,
}

/// Working directories the new-session picker no longer offers.
#[api(
    id = "gone_cwds",
    group = "sessions",
    summary = "Directories that are gone",
    responses(200 = "Working directories of this machine's sessions that are no longer directories, sorted, as looked at within the last half minute")
)]
pub async fn gone_cwds(State(state): State<DriveState>) -> Json<GoneCwds> {
    let drive = &state.drive;
    let cwds = drive
        .gone_dirs
        .answer(|| async {
            let bindings = drive.accounts.session_account_map().await;
            drive.transcripts.session_cwds(&drive.paths, &bindings).await
        })
        .await;
    Json(GoneCwds { cwds })
}

/// Composer model/effort/permission choices, shared across devices.
#[api(
    id = "composer_prefs",
    group = "prefs",
    summary = "Composer preferences",
    responses(200 = "Composer model/effort/permission choices, shared across devices")
)]
pub async fn composer_prefs(State(state): State<DriveState>) -> Json<ComposerPrefs> {
    Json(state.drive.composer.read().await)
}

/// Update the source default.
#[api(
    id = "put_composer_default",
    group = "prefs",
    summary = "Set a source's composer default",
    responses(200 = "Updated composer prefs (source default)"),
    drop_default
)]
pub async fn put_composer_default(
    State(state): State<DriveState>,
    ApiPath(source): ApiPath<Source>,
    ApiJson(body): ApiJson<ComposerChoice>,
) -> Result<Json<ComposerPrefs>, ApiError> {
    state
        .drive
        .composer
        .update(source, state.drive.providers.model_rule(source), None, &body)
        .await
        .map(Json)
        .map_err(|error| ApiError::bad_request(error.to_string()))
}

/// Update one session's entry, and the source default with it.
#[api(
    id = "put_composer_session",
    group = "prefs",
    summary = "Set a session's composer choice",
    responses(200 = "Updated composer prefs (session entry + source default)"),
    drop_default
)]
pub async fn put_composer_session(
    State(state): State<DriveState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiJson(body): ApiJson<ComposerChoice>,
) -> Result<Json<ComposerPrefs>, ApiError> {
    if id.chars().count() > 200 {
        return Err(ApiError::bad_request("session id is too long"));
    }
    state
        .drive
        .composer
        .update(source, state.drive.providers.model_rule(source), Some(&id), &body)
        .await
        .map(Json)
        .map_err(|error| ApiError::bad_request(error.to_string()))
}

impl DriveState {
    /// Register the session-driving routes.
    pub fn routes<S>(router: GotchaRouter<S>) -> GotchaRouter<S>
    where
        S: Clone + Send + Sync + 'static,
        DriveState: FromRef<S>,
    {
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
            .get("/api/sessions/{source}/{id}/permissions", session_permissions)
            .post("/api/sessions/{source}/{id}/permissions/{requestId}", respond_permission)
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
            .get("/api/sessions/gone-cwds", gone_cwds)
            .get("/api/composer-prefs", composer_prefs)
            .put("/api/composer-prefs/{source}", put_composer_default)
            .put("/api/composer-prefs/{source}/{id}", put_composer_session)
            .get("/api/agents", crate::agents::list_agents)
            .post("/api/agents/send", crate::agents::send_agent_message)
            .get("/api/agents/{source}/{id}/transcript", crate::agents::read_agent_transcript)
            .put("/api/sessions/{source}/{id}/account", bind_session_account)
            .post("/api/sessions/{source}/{id}/continue", continue_session_on_account)
    }
}
