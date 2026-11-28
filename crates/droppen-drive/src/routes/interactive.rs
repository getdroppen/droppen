//! Interactive agent question and tool permission routes.

use super::*;

// -------------------------------------------------------------------- questions

/// The questions the agent stopped to ask in this session, and what became of
/// them. The same list the session's `state` frames carry.
#[api(
    id = "session_questions",
    group = "sessions",
    summary = "Questions the agent asked",
    responses(200 = "The questions the agent asked, oldest first: the one it is blocked on and the recent ones with what became of them")
)]
pub async fn session_questions(
    State(state): State<DriveState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
) -> Result<Json<SessionQuestions>, ApiError> {
    let questions = if state.drive.providers.capabilities(source).questions {
        state.drive.injections.host.questions().for_session(&id, questions::now_ms())
    } else {
        Vec::new()
    };
    Ok(Json(SessionQuestions { questions }))
}

// ------------------------------------------------------------------ permissions

/// Path parameters for `POST /api/sessions/{source}/{id}/permissions/{requestId}`.
#[derive(Debug, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct PermissionPath {
    /// Session source (`claude`, `codex`, or `acp`).
    pub source: Source,
    /// Session id.
    pub id: String,
    /// Permission request id.
    pub request_id: String,
}

/// The tool permission prompts the agent asked in this session, oldest first.
#[api(
    id = "session_permissions",
    group = "sessions",
    summary = "Tool permission prompts the agent asked",
    responses(200 = "The tool permission prompts, oldest first: the pending ones and recent settled ones")
)]
pub async fn session_permissions(
    State(state): State<DriveState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
) -> Result<Json<SessionPermissions>, ApiError> {
    let permissions = if state.drive.providers.capabilities(source).permissions {
        state.drive.injections.host.permissions().for_session(&id, permissions::now_ms())
    } else {
        Vec::new()
    };
    Ok(Json(SessionPermissions { permissions }))
}

/// Approve, reject, or cancel a pending tool permission prompt.
#[api(
    id = "respond_permission",
    group = "sessions",
    summary = "Respond to a tool permission prompt",
    responses(200 = "The decision was delivered to the waiting agent"),
    errors(
        400 = "Missing or unknown optionId",
        404 = "No such permission prompt in this session",
        409 = "Already decided or cancelled (`permission_settled`, with the record as it stands)"
    )
)]
pub async fn respond_permission(
    State(state): State<DriveState>,
    #[api(skip)] MaybeIdentity(identity): MaybeIdentity,
    ApiPath(PermissionPath { source, id, request_id }): ApiPath<PermissionPath>,
    ApiJson(body): ApiJson<RespondPermission>,
) -> Result<Json<PermissionRespondResult>, ApiError> {
    let reply = if body.cancel.0 == Some(true) {
        PermissionReply::Cancel
    } else if let Some(option_id) = body.option_id.0.map(|id| id.trim().to_string()).filter(|id| !id.is_empty()) {
        PermissionReply::Select(option_id)
    } else {
        return Err(ApiError::bad_request("either optionId or cancel=true is required"));
    };
    if !state.drive.providers.capabilities(source).permissions {
        return Err(ApiError::not_found("no such permission prompt"));
    }
    writable_session(&state.drive, source, &id).await?;
    let store = state.drive.injections.host.permissions().clone();
    let stored = store
        .get(&request_id)
        .filter(|stored| stored.session_id == id)
        .ok_or_else(|| ApiError::not_found("no such permission prompt"))?;
    if stored.state != PermissionState::Pending {
        return Err(permission_already_settled(stored.wire()));
    }
    let by = identity.map(|identity| identity.username);
    let result = match state.drive.providers.respond_permission(source, &id, &request_id, reply, by).await {
        PermissionOutcome::Delivered(permission) => Ok(PermissionRespondResult { permission }),
        PermissionOutcome::Resolved(permission) => Err(permission_already_settled(permission)),
        PermissionOutcome::Invalid(why) => Err(ApiError::bad_request(why)),
        PermissionOutcome::NotReady => Err(ApiError::conflict(
            "the session is still reconnecting after a server restart; try again in a moment",
        )),
        PermissionOutcome::Unreachable => Err(ApiError::conflict(
            "the permission response could not be written to the agent; the prompt is still open",
        )),
        PermissionOutcome::Unknown | PermissionOutcome::NoResident => {
            store.close(&request_id, PermissionState::Cancelled, permissions::now_ms());
            match store.get(&request_id) {
                Some(current) => Err(permission_already_settled(current.wire())),
                None => Err(ApiError::not_found("no such permission prompt")),
            }
        }
    };
    if result.is_ok() {
        state.drive.injections.host.announce(source, &id);
    }
    result.map(Json)
}

#[derive(Serialize)]
struct PermissionSettled<'a> {
    error: &'static str,
    hint: &'a str,
    permission: SessionPermission,
}

pub(super) fn permission_already_settled(permission: SessionPermission) -> ApiError {
    let hint = match (permission.state, permission.answered_by.0.as_deref()) {
        (PermissionState::Resolved, Some(by)) => format!("{by} already decided this permission"),
        (PermissionState::Resolved, None) => "this permission was already decided".to_string(),
        (PermissionState::Cancelled, _) => "this permission prompt is no longer waiting".to_string(),
        (PermissionState::Pending, _) => "this permission prompt can no longer be answered".to_string(),
    };
    ApiError::refusal(
        StatusCode::CONFLICT,
        PermissionSettled {
            error: "permission_settled",
            hint: &hint,
            permission,
        },
    )
}

/// Answer a question the agent is waiting on.
///
/// Into the turn that asked when its process is still holding it, which is the
/// whole point: the agent carries on from the question with the answer in
/// hand. When that process is gone the question has *expired*, and the answer
/// is sent to the session as an ordinary message that says what it answers.
/// Either way, once: a second answer — from another screen, or a double tap —
/// is a `409` carrying the question as the first one left it.
#[api(
    id = "answer_question",
    group = "sessions",
    summary = "Answer a question the agent asked",
    responses(200 = "Answered: into the waiting turn (`turn`), or — the process that asked is gone — as a new message (`message`)"),
    errors(
        400 = "An answer keyed by something that is not one of the questions, or no answer at all",
        404 = "No such question in this session",
        409 = "Already answered, dismissed or withdrawn (`question_settled`, with the question as it stands), or the session cannot take it right now"
    )
)]
pub async fn answer_question(
    State(state): State<DriveState>,
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
#[api(
    id = "dismiss_question",
    group = "sessions",
    summary = "Dismiss a question the agent asked",
    responses(200 = "Dismissed: the agent was told to use its own judgement, or an expired question was closed"),
    errors(
        404 = "No such question in this session",
        409 = "Already answered, dismissed or withdrawn (`question_settled`, with the question as it stands)"
    )
)]
pub async fn dismiss_question(
    State(state): State<DriveState>,
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
pub(super) fn already_settled(question: SessionQuestion) -> ApiError {
    let hint = match (question.state, question.answered_by.0.as_deref()) {
        (QuestionState::Answered, Some(by)) => format!("{by} already answered it"),
        (QuestionState::Answered, None) => "it was already answered".to_string(),
        (QuestionState::Dismissed, _) => "it was already dismissed".to_string(),
        (QuestionState::Cancelled, _) => "the turn that asked it was stopped".to_string(),
        (QuestionState::Expired | QuestionState::Pending, _) => "it can no longer be answered".to_string(),
    };
    ApiError::refusal(
        gotcha::axum::http::StatusCode::CONFLICT,
        QuestionSettled {
            error: "question_settled",
            hint: &hint,
            question,
        },
    )
}

/// `question_settled`'s body: a refusal's code and hint, and the question.
#[derive(Serialize)]
struct QuestionSettled<'a> {
    error: &'static str,
    hint: &'a str,
    question: SessionQuestion,
}

/// Answer or dismiss one question, wherever it stands.
pub(super) async fn settle_question(
    drive: &crate::Drive,
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
    if !drive.providers.capabilities(source).questions {
        return Err(ApiError::not_found("no such question"));
    }
    let summary = writable_session(drive, source, id).await?;
    let store = drive.injections.host.questions().clone();
    let stored = store
        .get(request_id)
        .filter(|stored| stored.session_id == id)
        .ok_or_else(|| ApiError::not_found("no such question"))?;
    if let Reply::Answer(answer) = &reply {
        questions::check_answer(&stored.questions, answer).map_err(ApiError::bad_request)?;
    }
    let settled = match stored.state {
        QuestionState::Pending => match drive.providers.answer_question(source, id, request_id, reply.clone(), by.clone()).await {
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
    if let Ok(result) = &settled {
        if result.delivered != QuestionDelivery::None {
            drive.agents.reset_hops(source, id);
        }
        drive.injections.host.announce(source, id);
    }
    settled
}

/// Settle a question whose process is gone: dismissed, it is just closed;
/// answered, the answer goes to the session as an ordinary message.
///
/// The record moves first, as it does for a live question, so two screens
/// cannot both send the message; a send that fails puts it back.
async fn settle_expired(
    drive: &crate::Drive,
    summary: &droppen_sessions::SessionSummary,
    stored: &questions::Stored,
    reply: Reply,
    by: Option<String>,
) -> Result<QuestionAnswerResult, ApiError> {
    let store = drive.injections.host.questions();
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
    let (can_send, reason) = drive.transcripts.sendability(
        source,
        summary.scope,
        summary.status,
        drive.providers.is_running(source, id),
        drive.providers.has_resident(source, id),
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
    // An answer that has to become a message follows the same rule as a send:
    // a chat is pinned server-side and is never restarted with a permission
    // mode, whatever a stored composer row says.
    let chat = summary.cwd.as_ref().map(Path::new) == Some(chats_cwd(&drive.paths).as_path());
    let options = expired_answer_options(chat, choice);
    match drive.providers.send_to_session(source, id, summary.cwd.as_ref().map(String::as_str), &text, &options).await {
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

/// The options an expired question's answer travels under, once it has to
/// become a message.
///
/// A chat's `chat` marker must be set here like anywhere else — the provider
/// chooses inline content blocks over paths by it — and its permission mode
/// is dropped whatever a stored composer row says. A session keeps both.
pub(super) fn expired_answer_options(chat: bool, choice: ComposerChoice) -> SendOptions {
    SendOptions {
        chat,
        model: choice.model.0,
        effort: choice.effort.0,
        permission: if chat { None } else { choice.permission.0 },
        ..SendOptions::default()
    }
}
