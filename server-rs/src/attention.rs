//! The read marks' routes: `POST …/seen` for a session and for one of its
//! subagents. What a mark means, and the unread badge it feeds, are
//! [`crate::sessions::attention`]'s.

use std::sync::Arc;

use gotcha::axum::extract::FromRef;
use gotcha::{api, GotchaRouter, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use droppen_http::error::ApiError;
use droppen_http::extract::ApiPath;
use droppen_http::refresh::Refresh;
use droppen_http::openapi_ext::{ExtraResponses, Nullable};
use droppen_instance::paths::Paths;
use crate::sessions::attention::seen;
use crate::sessions::Source;

/// What the attention routes use of the server's state: where the read marks
/// are stored, and the feed told when one moves.
#[derive(Clone)]
pub struct AttentionState {
    /// The instance's paths, under which the read marks live.
    pub paths: Arc<Paths>,
    /// The global feed's inputs, told when a session's unread state changes.
    pub refresh: Refresh,
}
droppen_http::substate!(AttentionState);

/// Mark a session as read, as of now.
///
/// Deliberately does no existence check. The client calls this on every focus
/// and visibility change, and a lookup would mean scanning a projects tree to
/// answer a request whose whole job is to write eight words to a small file. An
/// id that names nothing costs one entry, which the store's own cap reclaims.
#[api(id = "mark_session_seen", group = "sessions", summary = "Mark a session read")]
pub async fn mark_seen(State(state): State<AttentionState>, ApiPath((source, id)): ApiPath<(Source, String)>) -> Result<Json<SeenResult>, ApiError> {
    let previous = seen::mark(state.paths.as_ref(), source, &id)
        .await
        .map_err(|error| ApiError::internal(format!("failed to record seen: {error}")))?;
    // The badge lives on the session-list rows, and those only change when the
    // feed re-lists — without this the badge would linger for up to the poll
    // interval after the user has plainly already looked at the session.
    state.refresh.request_refresh(None);
    Ok(Json(SeenResult {
        accepted: true,
        previous_read_at: Nullable(previous),
    }))
}

/// Mark one of a session's subagents as read, as of now.
///
/// For a subagent that is not a session of its own (Claude's), whose read mark
/// lives within its parent's: the tree under a session shows it unread until
/// it, or the session, is opened after it last wrote. A subagent that is a
/// session (a Codex thread) is marked through the session route.
#[api(id = "mark_agent_seen", group = "sessions", summary = "Mark a subagent read")]
pub async fn mark_agent_seen(
    State(state): State<AttentionState>,
    ApiPath(crate::sessions::sidecars::AgentPath { source, id, agent_id }): ApiPath<crate::sessions::sidecars::AgentPath>,
) -> Result<Json<SeenResult>, ApiError> {
    if !crate::sessions::sidecars::is_agent_id(&agent_id) {
        return Err(ApiError::bad_request("invalid agent id"));
    }
    let previous = seen::mark(state.paths.as_ref(), source, &crate::sessions::children::agent_seen_id(&id, &agent_id))
        .await
        .map_err(|error| ApiError::internal(format!("failed to record seen: {error}")))?;
    state.refresh.request_refresh(None);
    Ok(Json(SeenResult {
        accepted: true,
        previous_read_at: Nullable(previous),
    }))
}

/// What marking a session read answers.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SeenResult {
    /// Always true; the shape every other accept-only route answers with.
    pub accepted: bool,
    /// The mark this one replaced: when the user had last looked before now,
    /// ISO-8601, or `null` for a session never marked (or whose mark was
    /// evicted).
    ///
    /// The reader arriving at a session reports it seen at once, and that
    /// report is what moves the mark — so this is the only reliable place to
    /// learn where their unread messages began. A client hands it to
    /// `GET /api/sessions/{source}/{id}/first-unread?after=` to offer the way
    /// back to the first of them.
    pub previous_read_at: Nullable<String>,
}

impl AttentionState {
    /// Register this subsystem's routes.
    pub fn routes<S>(router: GotchaRouter<S>) -> GotchaRouter<S>
    where
        S: Clone + Send + Sync + 'static,
        AttentionState: FromRef<S>,
    {
        router
            .post("/api/sessions/{source}/{id}/seen", mark_seen)
            .post("/api/sessions/{source}/{id}/agents/{agentId}/seen", mark_agent_seen)
    }
}

/// Everything gotcha cannot express about those routes.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe_final("POST", "/api/sessions/{source}/{id}/seen", 200, "The session is now marked read")
        .describe_final(
            "POST",
            "/api/sessions/{source}/{id}/agents/{agentId}/seen",
            200,
            "The subagent is now marked read",
        );
}
