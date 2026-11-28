//! Session account binding and continue-on-account routes.

use super::*;

/// `PUT /api/sessions/{source}/{id}/account` body and response.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SessionAccount {
    /// The account this session runs under; `null` unbinds it.
    #[serde(deserialize_with = "required_nullable")]
    pub account_id: Nullable<String>,
}

/// `POST /api/sessions/{source}/{id}/continue` body.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SessionContinue {
    /// The stored claude account the new session runs under.
    pub account_id: String,
}

/// The session `POST /api/sessions/{source}/{id}/continue` made.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ContinuedSession {
    /// Which CLI it belongs to; always `claude`.
    pub source: Source,
    /// The new session's id. The session it was copied from keeps its own.
    pub id: String,
}

/// Pin a session to an account, or unbind it.
#[api(
    id = "bind_session_account",
    group = "accounts",
    summary = "Bind a session to an account",
    responses(200 = "Session pinned to an account (null unbinds it)"),
    errors(
        400 = "The account belongs to the other CLI",
        404 = "Session or account not found",
        409 = "The session keeps the local login, or is not this instance's to bind"
    )
)]
pub async fn bind_session_account(
    State(state): State<DriveState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiJson(body): ApiJson<SessionAccount>,
) -> Result<Json<SessionAccount>, ApiError> {
    let account_id = body.account_id.0;
    let Some(provider) = AccountProvider::of(source) else {
        return Err(ApiError::bad_request(
            "ACP sessions run on the agent's own configuration and have no account to bind",
        ));
    };
    if let Some(account_id) = account_id.as_deref() {
        account_id.require_uuid()?;
        // Before anything is read off disk: an account for the other CLI is a
        // client bug, and it is the same answer whatever the session turns out
        // to be.
        let Some(account) = state.drive.accounts.get(account_id).await else {
            return Err(ApiError::not_found("account not found"));
        };
        if account.provider != provider {
            return Err(provider_mismatch());
        }
    }
    if !state.drive.providers.capabilities(source).rotations {
        return bind_in_place_session(&state, source, provider, &id, account_id).await;
    }
    let found = state.drive.transcripts.find_root(&state.paths, source, &id).await;
    // Scoped to claude, because the binding table holds both CLIs' rows: a
    // codex thread id posted here would otherwise skip the not-found check on
    // the strength of its *codex* binding, and then overwrite it.
    let bound = state.drive.accounts.session_account_id_for(&id, provider).await;
    if found.is_none() && bound.is_none() {
        return Err(ApiError::not_found("session not found"));
    }
    // A session in the user's own root keeps the user's own login, always — and
    // that stays true of one *this server* started on the local login. An
    // account spawn runs against this instance's own Claude root, so the
    // transcript would have to be there, and nothing moves it there: copying it
    // would fork the conversation. The fork is a separate, explicit step —
    // [`continue_session_on_account`] — that makes a *new* session and leaves
    // this one as it was.
    if account_id.is_some() && found == Some(ClaudeRoot::Local) {
        return Err(ApiError::refusal(
            gotcha::axum::http::StatusCode::CONFLICT,
            Hinted {
                error: "local_session_cannot_use_account",
                hint: "this session runs on the local login and its transcript is in ~/.claude, \
                       so it cannot move onto an account. Continue it on the account instead, \
                       which copies the conversation into a new session.",
            },
        ));
    }
    write_binding(&state, source, &id, account_id).await
}

/// Carry a local-login claude session onto an account, as a new session.
///
/// The answer to `local_session_cannot_use_account`, for the moment it matters:
/// the local login stopped working and the conversation is stuck on it. The
/// session's transcript is copied into this instance's own root under a new id
/// (`sessions::continuation`), the copy is bound to the account, and
/// the first message sent to it resumes the whole conversation there. The
/// original is read and never written; it stays as it was, on the local login.
///
/// Only a session this server started (`external_session` otherwise) and only
/// one on the local login: a session already in this instance's root switches
/// accounts in place, and so does every codex thread.
#[api(
    id = "continue_session_on_account",
    group = "accounts",
    summary = "Continue a local-login session on an account",
    responses(200 = "A new session on the account, holding a copy of this conversation; the original is untouched"),
    errors(
        400 = "A codex session, or an account for the other CLI",
        404 = "Session or account not found",
        409 = "Not this instance's session, or not one on the local login",
        500 = "The conversation could not be copied"
    )
)]
pub async fn continue_session_on_account(
    State(state): State<DriveState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiJson(body): ApiJson<SessionContinue>,
) -> Result<Json<ContinuedSession>, ApiError> {
    let Some(provider) = AccountProvider::of(source) else {
        return Err(continue_not_supported(source));
    };
    if !state.drive.providers.capabilities(source).rotations {
        return Err(continue_not_supported(source));
    }
    body.account_id.require_uuid()?;
    let Some(account) = state.drive.accounts.get(&body.account_id).await else {
        return Err(account_not_found());
    };
    if account.provider != provider {
        return Err(provider_mismatch());
    }
    // The same row, and the same ownership verdict, the list and the composer
    // were given: somebody else's session is not this server's to copy.
    let summary = crate::routes::writable_session(&state.drive, source, &id).await?;
    if summary.claude_root != Some(ClaudeRoot::Local) {
        return Err(ApiError::refusal(
            StatusCode::CONFLICT,
            Hinted {
                error: "session_not_on_local_login",
                hint: "this session already runs in this server's own root; switch its account instead",
            },
        ));
    }

    let new_id = uuid::Uuid::new_v4().to_string();
    let copy = state.drive.providers.continue_on_account(source, &id, &new_id).await.map_err(|error| match error {
        ContinueError::Unsupported => continue_not_supported(source),
        ContinueError::NotFound => ApiError::not_found("session not found"),
        ContinueError::Taken => ApiError::conflict(error.to_string()),
        ContinueError::Io(_) => {
            tracing::warn!(session = %id, %error, "cannot copy a local-login session into this instance's root");
            ApiError::internal("cannot copy the conversation")
        }
    })?;
    if let Err(error) = state
        .drive
        .accounts
        .bind_session_account(&new_id, Some(&account.id), provider)
        .await
    {
        // A copy with no account would list as a managed session that refuses
        // every message; better no copy at all.
        tracing::warn!(session = %id, continued = %new_id, %error, "cannot bind the continued session; removing the copy");
        let _ = tokio::fs::remove_file(&copy).await;
        if let Some(project) = copy.parent() {
            let _ = tokio::fs::remove_dir_all(project.join(&new_id)).await;
        }
        return Err(ApiError::not_found("cannot bind account"));
    }
    // A person picked this account: nothing moves it ahead of a limit for a while.
    state
        .drive
        .accounts
        .rotation_log()
        .note_hand_binding(&new_id, droppen_accounts::rest::now_ms());
    // A name somebody gave the conversation goes with it.
    let titles = state.drive.titles.read().await;
    if let Some(title) = titles.get(&droppen_sessions::reference::title_key(source, &id)) {
        if let Err(error) = state.drive.titles.set(source, &new_id, title).await {
            tracing::warn!(session = %new_id, %error, "cannot carry the title over to the continued session");
        }
    }
    tracing::info!(session = %id, continued = %new_id, account = %account.id, "continued a local-login session on an account as a new session");
    state.refresh.request_refresh(Some((&source, &new_id)));
    Ok(Json(ContinuedSession { source, id: new_id }))
}

/// Continuing a session on an account is refused for its CLI.
fn continue_not_supported(source: Source) -> ApiError {
    let hint = if AccountProvider::of(source).is_some() {
        "a codex session switches accounts in place; bind it to the account instead"
    } else {
        "this session runs on the agent's own configuration and does not use a Droppen account"
    };
    ApiError::refusal(
        StatusCode::BAD_REQUEST,
        Hinted {
            error: "continue_not_supported",
            hint,
        },
    )
}

/// The codex half of the route above.
///
/// Ownership rather than a config root, because codex has no second root to
/// own: a thread this server did not start is one the user is driving
/// somewhere else, and re-pointing its login from here would change which
/// account the *other* driver bills. The refusal is the send route's own, so a
/// client that already renders `external_session` renders this too.
async fn bind_in_place_session(
    state: &DriveState,
    source: Source,
    provider: AccountProvider,
    id: &str,
    account_id: Option<String>,
) -> Result<Json<SessionAccount>, ApiError> {
    let bound = state.drive.accounts.session_account_id_for(id, provider).await;
    match crate::routes::writable_session(&state.drive, source, id).await {
        Ok(_) => {}
        // A session with no transcript yet but a binding already written is one
        // this server is in the middle of starting; it is ours by construction.
        Err(_) if bound.is_some() => {}
        Err(error) => return Err(error),
    }
    write_binding(state, source, id, account_id).await
}

/// Store the binding and tell the feed the row changed.
async fn write_binding(state: &DriveState, source: Source, id: &str, account_id: Option<String>) -> Result<Json<SessionAccount>, ApiError> {
    let provider = AccountProvider::of(source).ok_or_else(|| ApiError::bad_request("this session has no account to bind"))?;
    state
        .drive
        .accounts
        .bind_session_account(id, account_id.as_deref(), provider)
        .await
        // Both arms are a 404 there: the route reports `err.message` for an
        // `AccountError` and its own fallback for anything else.
        .map_err(|error| match error {
            AccountError::ProviderMismatch { .. } => provider_mismatch(),
            AccountError::NotFound(_) => ApiError::not_found(error.to_string()),
            _ => ApiError::not_found("cannot bind account"),
        })?;
    // Somebody chose this account for the session: nothing moves it ahead of a
    // limit until the choice has stood for a while (see
    // [`droppen_accounts::rotation::HAND_BINDING_GRACE_MS`]).
    if state.drive.providers.capabilities(source).rotations && account_id.is_some() {
        state.drive.accounts.rotation_log().note_hand_binding(id, droppen_accounts::rest::now_ms());
    }
    // The binding shows on the session row, so the list has to be re-read.
    state.refresh.request_refresh(Some((&source, id)));
    Ok(Json(SessionAccount {
        account_id: Nullable(account_id),
    }))
}
