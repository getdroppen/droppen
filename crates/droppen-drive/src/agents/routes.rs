//! Loopback HTTP handlers for the local agent bus.
//!
//! Mounted as plain axum routes rather than `#[api]` operations on purpose:
//! these three endpoints are an internal IPC surface for `droppen mcp-agents`
//! child processes on `127.0.0.1` (blocked from the cloud bridge as `Never` and
//! rejected without an `AgentsGrant`), not a browser-facing REST API, so they
//! deliberately stay out of the public `openapi.json` contract.

use std::path::Path;
use std::time::Instant;

use gotcha::axum::http::StatusCode;
use gotcha::{Json, State};
use serde::{Deserialize, Serialize};

use droppen_http::error::ApiError;
use droppen_http::extract::{ApiJson, ApiPath, ApiQuery};
use droppen_instance::paths::Paths;
use droppen_provider::SendOptions;
use droppen_sessions::attention::seen::key_of;
use droppen_sessions::util::{ItemKind, Role};
use droppen_sessions::{chats_cwd, file_signature, ScopeFilter, SessionScope, SessionStatus, SessionSummary, Source};

use crate::feed;
use crate::routes::DriveState;

use super::bus::{wrap_cross_session_message, ThrottleError, MAX_HOPS};
use super::token::AgentsCaller;

/// List owned non-chat sessions visible on the local agent bus.
pub const AGENTS_LIST_PATH: &str = "/api/agents";
/// Deliver a cross-session message to another session.
pub const AGENTS_SEND_PATH: &str = "/api/agents/send";
/// Read a trimmed transcript summary of another session.
pub const AGENTS_TRANSCRIPT_PATH: &str = "/api/agents/{source}/{id}/transcript";

/// Maximum body size of one cross-session message in bytes.
const MAX_MESSAGE_BYTES: usize = 16 * 1024;
/// Default and ceiling for `list_agents`.
const DEFAULT_LIST_LIMIT: usize = 20;
const MAX_LIST_LIMIT: usize = 50;
/// Default and ceiling for `read_session`.
const DEFAULT_TRANSCRIPT_LIMIT: usize = 10;
const MAX_TRANSCRIPT_LIMIT: usize = 30;
/// Per-item character cap in `read_session`.
const MAX_ITEM_CHARS: usize = 2_000;
/// Overall response text budget in `read_session`.
const MAX_TRANSCRIPT_BYTES: usize = 16 * 1024;

/// Query parameters for `GET /api/agents`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAgentsQuery {
    /// Optional working directory prefix filter.
    pub cwd: Option<String>,
    /// Optional provider source filter (`claude`, `codex`, `acp`).
    pub source: Option<Source>,
    /// Only return currently active/running sessions.
    pub active_only: Option<bool>,
    /// Maximum number of peers returned (default 20, max 50).
    pub limit: Option<usize>,
}

/// Summary of one session on the local agent bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPeer {
    /// Canonical address (`<source>/<id>`).
    #[serde(rename = "ref")]
    pub peer_ref: String,
    /// Provider source.
    pub source: Source,
    /// Session id.
    pub id: String,
    /// Configured ACP agent id when `source == acp`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acp_agent: Option<String>,
    /// Claude Code CLI peer name (`@name`) while running, if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Session title.
    pub title: String,
    /// Working directory, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Live session status.
    pub status: SessionStatus,
    /// Fine-grained activity (`working` / `waiting` / `idle`).
    pub activity: droppen_sessions::SessionActivity,
    /// Whether this session can currently accept a cross-session message.
    pub can_receive: bool,
    /// Last updated timestamp (ISO-8601).
    pub updated_at: String,
}

/// Response of `GET /api/agents`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAgentsResponse {
    /// The calling session's own descriptor.
    #[serde(rename = "self")]
    pub caller: AgentPeer,
    /// Other owned non-chat sessions on this Droppen instance.
    pub agents: Vec<AgentPeer>,
}

/// Request body of `POST /api/agents/send`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendAgentMessageBody {
    /// Target `<source>/<id>` ref or unique Claude `@agentName`.
    pub target: String,
    /// Plain-text message to deliver.
    pub message: String,
}

/// Response of `POST /api/agents/send`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendAgentMessageResponse {
    /// Always `true` on HTTP 200.
    pub accepted: bool,
    /// Whether the message was queued behind an in-flight turn.
    pub queued: bool,
    /// Resolved target `<source>/<id>` ref.
    pub target: String,
    /// Consecutive cross-session hop count stamped on this delivery.
    pub hop: u32,
}

/// Query parameters for `GET /api/agents/{source}/{id}/transcript`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadAgentTranscriptQuery {
    /// Maximum number of recent items to return (default 10, max 30).
    pub limit: Option<usize>,
    /// Include compact tool-use summaries (`toolName` + one-line `text`).
    pub include_tools: Option<bool>,
}

/// One trimmed transcript item returned by `read_session`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTranscriptItem {
    /// Role (`user`, `assistant`, `system`).
    pub role: Role,
    /// Item kind (`text`, `tool_use`, `compact`, `command`).
    pub kind: ItemKind,
    /// Trimmed text body or tool summary.
    pub text: String,
    /// Tool name when `kind == tool_use`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Whether the item represents an error.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
    /// ISO-8601 timestamp when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// Response of `GET /api/agents/{source}/{id}/transcript`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadAgentTranscriptResponse {
    /// Target `<source>/<id>` ref.
    pub target: String,
    /// Provider source.
    pub source: Source,
    /// Session id.
    pub id: String,
    /// Session title.
    pub title: String,
    /// Working directory, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Live status.
    pub status: SessionStatus,
    /// Fine-grained activity.
    pub activity: droppen_sessions::SessionActivity,
    /// Whether the session is blocked waiting on a human permission approval.
    pub pending_permission: bool,
    /// Whether the session is blocked waiting on a human question answer.
    pub pending_question: bool,
    /// Recent trimmed transcript items in chronological order.
    pub items: Vec<AgentTranscriptItem>,
}

fn is_chat(paths: &Paths, summary: &SessionSummary) -> bool {
    summary.cwd.0.as_deref().map(Path::new) == Some(chats_cwd(paths).as_path())
}

fn can_receive(drive: &crate::Drive, summary: &SessionSummary) -> bool {
    if summary.scope != SessionScope::Owned || is_chat(&drive.paths, summary) {
        return false;
    }
    drive.providers.send_block(summary).is_none()
}

fn to_peer(drive: &crate::Drive, summary: &SessionSummary) -> AgentPeer {
    AgentPeer {
        peer_ref: key_of(summary.source, &summary.id),
        source: summary.source,
        id: summary.id.clone(),
        acp_agent: summary.acp_agent.clone(),
        agent_name: summary.agent_name.clone(),
        title: summary.title.clone(),
        cwd: summary.cwd.0.clone(),
        status: summary.status,
        activity: summary.activity,
        can_receive: can_receive(drive, summary),
        updated_at: summary.updated_at.clone(),
    }
}

fn fallback_self_peer(caller: &AgentsCaller) -> AgentPeer {
    AgentPeer {
        peer_ref: caller.key(),
        source: caller.source(),
        id: caller.id().to_string(),
        acp_agent: None,
        agent_name: None,
        title: caller.key(),
        cwd: None,
        status: SessionStatus::Running,
        activity: droppen_sessions::SessionActivity::Working,
        can_receive: true,
        updated_at: String::new(),
    }
}

fn sender_display_name(caller: &AgentsCaller, summary: Option<&SessionSummary>) -> String {
    if let Some(summary) = summary {
        if let Some(name) = summary.agent_name.as_deref().filter(|s| !s.is_empty()) {
            return name.to_string();
        }
        let prefix = summary
            .acp_agent
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| caller.source().as_str());
        let title = summary.title.trim();
        if !title.is_empty() {
            let short: String = title.chars().take(48).collect();
            return format!("{prefix}: {short}");
        }
        return prefix.to_string();
    }
    caller.key()
}

fn resolve_target<'a>(target: &str, sessions: &'a [SessionSummary]) -> Result<&'a SessionSummary, ApiError> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request("target is required"));
    }
    if let Some(parsed) = AgentsCaller::parse(trimmed) {
        return sessions
            .iter()
            .find(|s| s.source == parsed.source() && s.id == parsed.id())
            .ok_or_else(|| ApiError::not_found(format!("target session not found: {trimmed}")));
    }
    let name = trimmed.strip_prefix('@').unwrap_or(trimmed);
    let matches: Vec<&SessionSummary> = sessions
        .iter()
        .filter(|s| s.agent_name.as_deref() == Some(name))
        .collect();
    match matches.as_slice() {
        [only] => Ok(*only),
        [] => Err(ApiError::not_found(format!("target agent not found: {trimmed}"))),
        many => {
            let refs = many
                .iter()
                .map(|s| key_of(s.source, &s.id))
                .collect::<Vec<_>>()
                .join(", ");
            Err(ApiError::conflict(format!(
                "multiple sessions share agent name '{name}'; specify one of: {refs}"
            )))
        }
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// `GET /api/agents` — list owned non-chat sessions visible on the local bus.
pub async fn list_agents(
    State(state): State<DriveState>,
    caller: AgentsCaller,
    ApiQuery(query): ApiQuery<ListAgentsQuery>,
) -> Result<Json<ListAgentsResponse>, ApiError> {
    let drive = &state.drive;
    let limit = query.limit.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, MAX_LIST_LIMIT);
    let active_only = query.active_only.unwrap_or(false);

    let bindings = drive.accounts.session_account_map().await;
    let drivers = drive.providers.driver_states();
    let sessions = drive
        .transcripts
        .list_and_filter(&drive.paths, &bindings, &drivers, None, None, ScopeFilter::Owned)
        .await;

    let caller_peer = sessions
        .iter()
        .find(|s| s.source == caller.source() && s.id == caller.id())
        .map(|s| to_peer(drive, s))
        .unwrap_or_else(|| fallback_self_peer(&caller));

    let mut peers: Vec<AgentPeer> = sessions
        .iter()
        .filter(|s| !(s.source == caller.source() && s.id == caller.id()))
        .filter(|s| !is_chat(&drive.paths, s))
        .filter(|s| query.source.is_none_or(|wanted| s.source == wanted))
        .filter(|s| {
            query
                .cwd
                .as_deref()
                .filter(|c| !c.is_empty())
                .is_none_or(|prefix| s.cwd.0.as_deref().is_some_and(|cwd| cwd.starts_with(prefix)))
        })
        .filter(|s| !active_only || s.status != SessionStatus::Idle)
        .map(|s| to_peer(drive, s))
        .collect();

    // Active sessions sort ahead of idle ones while preserving recency order inside each group.
    peers.sort_by_key(|p| p.status == SessionStatus::Idle);
    peers.truncate(limit);

    Ok(Json(ListAgentsResponse {
        caller: caller_peer,
        agents: peers,
    }))
}

/// `POST /api/agents/send` — deliver a cross-session message to another session.
pub async fn send_agent_message(
    State(state): State<DriveState>,
    caller: AgentsCaller,
    ApiJson(body): ApiJson<SendAgentMessageBody>,
) -> Result<Json<SendAgentMessageResponse>, ApiError> {
    let drive = &state.drive;
    let message = body.message.trim();
    if message.is_empty() {
        return Err(ApiError::bad_request("message is empty"));
    }
    if message.len() > MAX_MESSAGE_BYTES {
        return Err(ApiError::bad_request(format!(
            "message exceeds the {} KiB limit",
            MAX_MESSAGE_BYTES / 1024
        )));
    }

    let bindings = drive.accounts.session_account_map().await;
    let drivers = drive.providers.driver_states();
    let sessions = drive
        .transcripts
        .list_and_filter(&drive.paths, &bindings, &drivers, None, None, ScopeFilter::Owned)
        .await;

    let target = resolve_target(&body.target, &sessions)?;
    let target_ref = key_of(target.source, &target.id);
    let caller_ref = caller.key();
    if target.source == caller.source() && target.id == caller.id() {
        return Err(ApiError::bad_request("cannot send a message to yourself"));
    }
    if !can_receive(drive, target) {
        return Err(ApiError::conflict(
            drive
                .providers
                .send_block(target)
                .unwrap_or_else(|| "target session cannot receive cross-session messages".to_string()),
        ));
    }

    let (can_send, reason) = drive.transcripts.sendability(
        target.source,
        target.scope,
        target.status,
        drive.providers.is_running(target.source, &target.id),
        drive.providers.has_resident(target.source, &target.id),
    );
    if !can_send {
        return Err(ApiError::conflict(
            reason.unwrap_or_else(|| "target session cannot accept input right now".to_string()),
        ));
    }

    let hop = drive
        .agents
        .check_and_record(&caller_ref, &target_ref, Instant::now())
        .map_err(|err| match err {
            ThrottleError::RateLimited => ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "cross-session rate limit reached for this target; wait before sending again",
            ),
            ThrottleError::HopLimitExceeded => ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                format!("cross-session hop limit ({MAX_HOPS}) reached; pause and let the user intervene"),
            ),
        })?;

    let caller_summary = sessions
        .iter()
        .find(|s| s.source == caller.source() && s.id == caller.id());
    let from_name = sender_display_name(&caller, caller_summary);
    let wrapped = wrap_cross_session_message(&caller_ref, &from_name, hop, message);

    // Default `SendOptions` keeps the target's own account, model and
    // permission mode untouched and never routes through `answer_question`:
    // another agent must never approve a tool permission or answer a question
    // meant for a person.
    let sent = drive
        .providers
        .send_to_session(
            target.source,
            &target.id,
            target.cwd.0.as_deref(),
            &wrapped,
            &SendOptions::default(),
        )
        .await
        .map_err(|error| ApiError::conflict(error.client_message("failed to deliver cross-session message")))?;

    state.global.request_refresh(Some((target.source, &target.id)));
    tracing::info!(from = %caller_ref, to = %target_ref, hop, queued = sent.queued, "agents: delivered cross-session message");

    Ok(Json(SendAgentMessageResponse {
        accepted: true,
        queued: sent.queued,
        target: target_ref,
        hop,
    }))
}

/// `GET /api/agents/{source}/{id}/transcript` — read trimmed recent transcript items.
pub async fn read_agent_transcript(
    State(state): State<DriveState>,
    _caller: AgentsCaller,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiQuery(query): ApiQuery<ReadAgentTranscriptQuery>,
) -> Result<Json<ReadAgentTranscriptResponse>, ApiError> {
    let drive = &state.drive;
    let limit = query.limit.unwrap_or(DEFAULT_TRANSCRIPT_LIMIT).clamp(1, MAX_TRANSCRIPT_LIMIT);
    let include_tools = query.include_tools.unwrap_or(false);

    let not_found = || ApiError::not_found("session not found");
    let state_now = feed::load_session_state(drive, source, &id).await.ok_or_else(not_found)?;
    if state_now.summary.scope != SessionScope::Owned || is_chat(&drive.paths, &state_now.summary) {
        return Err(not_found());
    }

    let file_path = Path::new(&state_now.summary.file_path);
    let sig = file_signature(file_path).await.ok_or_else(not_found)?;
    let transcript = drive.transcripts.load(&drive.paths, source, file_path, &sig).await;

    let mut filtered: Vec<AgentTranscriptItem> = transcript
        .items
        .iter()
        .filter(|item| match item.kind {
            ItemKind::Text | ItemKind::Compact | ItemKind::Command => true,
            ItemKind::ToolUse => include_tools,
            ItemKind::Thinking | ItemKind::ToolResult => false,
        })
        .map(|item| AgentTranscriptItem {
            role: item.role,
            kind: item.kind,
            text: truncate_chars(&item.text, MAX_ITEM_CHARS),
            tool_name: (item.kind == ItemKind::ToolUse).then(|| item.tool_name.0.clone()).flatten(),
            is_error: item.is_error,
            timestamp: item.timestamp.0.clone(),
        })
        .collect();

    if filtered.len() > limit {
        filtered = filtered.split_off(filtered.len() - limit);
    }

    // Spend the response text budget from newest backwards so the peer always
    // sees the latest state even on a long turn.
    let mut kept_rev = Vec::with_capacity(filtered.len());
    let mut used = 0usize;
    for item in filtered.into_iter().rev() {
        let len = item.text.len() + 64;
        if !kept_rev.is_empty() && used + len > MAX_TRANSCRIPT_BYTES {
            break;
        }
        used += len;
        kept_rev.push(item);
    }
    kept_rev.reverse();

    let pending_permission = state_now
        .permissions
        .iter()
        .any(|p| p.state == droppen_sessions::delivery::PermissionState::Pending);
    let pending_question = state_now
        .questions
        .iter()
        .any(|q| q.state == droppen_sessions::delivery::QuestionState::Pending);

    Ok(Json(ReadAgentTranscriptResponse {
        target: key_of(source, &id),
        source,
        id,
        title: state_now.summary.title,
        cwd: state_now.summary.cwd.0,
        status: state_now.summary.status,
        activity: state_now.summary.activity,
        pending_permission,
        pending_question,
        items: kept_rev,
    }))
}
