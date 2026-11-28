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

fn resolve_target<'a>(target: &str, sessions: &[&'a SessionSummary]) -> Result<&'a SessionSummary, ApiError> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request("target is required"));
    }
    if let Some(parsed) = AgentsCaller::parse(trimmed) {
        return sessions
            .iter()
            .copied()
            .find(|s| s.source == parsed.source() && s.id == parsed.id())
            .ok_or_else(|| ApiError::not_found(format!("target session not found: {trimmed}")));
    }
    let name = trimmed.strip_prefix('@').unwrap_or(trimmed);
    let matches: Vec<&SessionSummary> = sessions
        .iter()
        .copied()
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

/// Verify that the authenticated caller belongs to an active, owned, non-chat
/// session on this Droppen instance.
///
/// Rejects archived, deleted, chat, and external/unowned sessions with
/// `403 Forbidden`. A freshly spawned session whose first transcript line has
/// not landed on disk yet is admitted only while it has a live resident
/// registered and is not explicitly archived.
async fn validate_caller(
    drive: &crate::Drive,
    caller: &AgentsCaller,
    listed_owned: Option<&[SessionSummary]>,
) -> Result<Option<SessionSummary>, ApiError> {
    let forbidden = || ApiError::new(StatusCode::FORBIDDEN, "caller session is not allowed on the agent bus");
    if let Some(sessions) = listed_owned {
        if let Some(summary) = sessions
            .iter()
            .find(|s| s.source == caller.source() && s.id == caller.id() && !is_chat(&drive.paths, s))
        {
            return Ok(Some(summary.clone()));
        }
    }
    if let Some(summary) = drive.transcripts.find_summary(&drive.paths, caller.source(), caller.id()).await {
        if summary.scope == SessionScope::Owned && !is_chat(&drive.paths, &summary) {
            return Ok(Some(summary));
        }
        return Err(forbidden());
    }
    if !drive.transcripts.is_archived(&drive.paths, caller.source(), caller.id()).await
        && drive.providers.has_resident(caller.source(), caller.id())
    {
        return Ok(None);
    }
    Err(forbidden())
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

    let caller_peer = validate_caller(drive, &caller, Some(&sessions))
        .await?
        .map(|s| to_peer(drive, &s))
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
                .is_none_or(|prefix| {
                    s.cwd
                        .0
                        .as_deref()
                        .is_some_and(|cwd| Path::new(cwd).starts_with(Path::new(prefix)))
                })
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
    let bindings = drive.accounts.session_account_map().await;
    let drivers = drive.providers.driver_states();
    let sessions = drive
        .transcripts
        .list_and_filter(&drive.paths, &bindings, &drivers, None, None, ScopeFilter::Owned)
        .await;

    let caller_summary = validate_caller(drive, &caller, Some(&sessions)).await?;

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

    let eligible_targets: Vec<&SessionSummary> = sessions
        .iter()
        .filter(|s| !is_chat(&drive.paths, s))
        .collect();
    let target = resolve_target(&body.target, &eligible_targets)?;
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

    let reservation = drive
        .agents
        .reserve(&caller_ref, &target_ref, Instant::now())
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
    let hop = reservation.hop();

    let from_name = sender_display_name(&caller, caller_summary.as_ref());
    let wrapped = wrap_cross_session_message(&caller_ref, &from_name, hop, message);

    // Default `SendOptions` keeps the target's own account, model and
    // permission mode untouched and never routes through `answer_question`:
    // another agent must never approve a tool permission or answer a question
    // meant for a person.
    let sent = match drive
        .providers
        .send_to_session(
            target.source,
            &target.id,
            target.cwd.0.as_deref(),
            &wrapped,
            &SendOptions::default(),
        )
        .await
    {
        Ok(sent) => {
            reservation.commit();
            sent
        }
        Err(error) => {
            reservation.rollback();
            return Err(ApiError::conflict(
                error.client_message("failed to deliver cross-session message"),
            ));
        }
    };

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
    caller: AgentsCaller,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiQuery(query): ApiQuery<ReadAgentTranscriptQuery>,
) -> Result<Json<ReadAgentTranscriptResponse>, ApiError> {
    let drive = &state.drive;
    validate_caller(drive, &caller, None).await?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use droppen_accounts::login::Logins;
    use droppen_auth::{AgentsCredential, Auth, NotifyCredential};
    use droppen_http::config::AuthSettings;
    use droppen_http::identity::{AgentsGrant, NotifyGrant};
    use droppen_instance::instance::{InstanceDirs, DEFAULT_INSTANCE};
    use droppen_instance::paths::ClaudeRoot;
    use droppen_provider::models::{ModelList, Models, ModelsQuery};
    use droppen_provider::{
        AnswerOutcome, Capabilities, ContinueError, DriverStatesBuilder, InjectError, InjectionRegistry, Phase,
        Provider, ProviderRegistry, ProviderRuns, QueueOutcome, Reply, ResidentState, Run, RunHost, SendTo, Sent,
        SpawnGate, StartAt,
    };
    use droppen_sessions::context::ContextUsage;
    use droppen_sessions::delivery::QuestionState;
    use droppen_sessions::util::TranscriptItem;
    use droppen_sessions::{CliChoice, Injection, TranscriptSources, Transcripts};
    use futures_util::future::BoxFuture;
    use serde_json::{Map, Value};

    use crate::routes::{answer_question, send_message, AnswerQuestion, AnswerValue, QuestionPath, SendMessage};
    use crate::Drive;

    struct NoNotify;
    impl NotifyCredential for NoNotify {
        fn is_notify_request(&self, _: &gotcha::axum::http::Method, _: &str) -> bool {
            false
        }
        fn caller<'a>(
            &'a self,
            _: &'a Paths,
            _: &'a str,
            _: &'a gotcha::axum::http::HeaderMap,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<NotifyGrant>> + Send + 'a>> {
            Box::pin(std::future::ready(None))
        }
    }

    struct NoAgents;
    impl AgentsCredential for NoAgents {
        fn is_agents_request(&self, _: &gotcha::axum::http::Method, _: &str) -> bool {
            false
        }
        fn caller<'a>(
            &'a self,
            _: &'a Paths,
            _: &'a str,
            _: &'a gotcha::axum::http::HeaderMap,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<AgentsGrant>> + Send + 'a>> {
            Box::pin(std::future::ready(None))
        }
    }

    #[derive(Clone)]
    struct MockSession {
        id: String,
        scope: SessionScope,
        cwd: String,
        file: PathBuf,
    }

    #[derive(Default)]
    struct HarnessConfig {
        sessions: HashMap<String, MockSession>,
        archived: HashSet<String>,
        residents: HashSet<String>,
        open_questions: HashMap<String, String>,
        fail_send: bool,
        delivered_texts: Vec<(String, String)>,
    }

    struct MockReader {
        cfg: Arc<Mutex<HarnessConfig>>,
    }

    impl MockReader {
        fn summary(&self, id: &str) -> Option<SessionSummary> {
            let cfg = self.cfg.lock().unwrap();
            if cfg.archived.contains(id) {
                return None;
            }
            let s = cfg.sessions.get(id)?;
            let size = std::fs::metadata(&s.file).ok()?.len();
            let row = serde_json::json!({
                "pinned": false,
                "id": s.id,
                "source": "claude",
                "scope": match s.scope { SessionScope::Owned => "owned", SessionScope::External => "external" },
                "server": "local",
                "title": format!("session {}", s.id),
                "cwd": s.cwd,
                "repoKey": null,
                "gitBranch": null,
                "updatedAt": "2026-10-10T00:00:00.000Z",
                "createdAt": "2026-10-10T00:00:00.000Z",
                "sizeBytes": size,
                "filePath": s.file.to_string_lossy(),
                "status": "idle",
                "activity": "idle",
                "resident": null,
                "claudeRoot": null,
                "accountId": null,
                "attentionAt": null,
                "unread": false,
                "unreadCount": 0,
                "acpAgent": null
            });
            Some(serde_json::from_value(row).expect("summary"))
        }
    }

    impl Transcripts for MockReader {
        fn source(&self) -> Source {
            Source::Claude
        }
        fn list<'a>(&'a self, _: &'a Paths, _: &'a BTreeMap<String, String>) -> BoxFuture<'a, Vec<SessionSummary>> {
            let ids: Vec<String> = self.cfg.lock().unwrap().sessions.keys().cloned().collect();
            let out = ids.into_iter().filter_map(|id| self.summary(&id)).collect();
            Box::pin(std::future::ready(out))
        }
        fn find<'a>(&'a self, _: &'a Paths, id: &'a str) -> BoxFuture<'a, Option<SessionSummary>> {
            Box::pin(std::future::ready(self.summary(id)))
        }
        fn is_archived<'a>(&'a self, _: &'a Paths, id: &'a str) -> BoxFuture<'a, bool> {
            let archived = self.cfg.lock().unwrap().archived.contains(id);
            Box::pin(std::future::ready(archived))
        }
        fn find_root<'a>(&'a self, _: &'a Paths, _: &'a str) -> BoxFuture<'a, Option<ClaudeRoot>> {
            Box::pin(std::future::ready(None))
        }
        fn parse<'a>(&'a self, _: &'a Paths, file_path: &'a Path) -> BoxFuture<'a, std::io::Result<Vec<TranscriptItem>>> {
            Box::pin(async move {
                let text = tokio::fs::read_to_string(file_path).await?;
                Ok(text.lines().map(|l| TranscriptItem::user_text(l, None)).collect())
            })
        }
        fn cli_choice<'a>(&'a self, _: &'a Path) -> BoxFuture<'a, Option<CliChoice>> {
            Box::pin(std::future::ready(None))
        }
        fn context<'a>(&'a self, _: &'a Path) -> BoxFuture<'a, Option<ContextUsage>> {
            Box::pin(std::future::ready(None))
        }
        fn spoken_texts(&self, _: &Map<String, Value>) -> Vec<String> {
            Vec::new()
        }
        fn external_reason(&self) -> String {
            "external".to_string()
        }
        fn live_block(&self, _: SessionStatus) -> Option<String> {
            None
        }
        fn foreign_sessions(&self) -> bool {
            false
        }
        fn on_owned<'a>(&'a self, _: &'a Paths, _: &'a str) -> BoxFuture<'a, ()> {
            Box::pin(std::future::ready(()))
        }
        fn watch_dirs<'a>(&'a self, _: &'a Paths) -> BoxFuture<'a, Vec<PathBuf>> {
            Box::pin(std::future::ready(Vec::new()))
        }
        fn has_own_sessions<'a>(&'a self, _: &'a Paths) -> BoxFuture<'a, bool> {
            Box::pin(std::future::ready(true))
        }
    }

    struct MockRuns {
        host: Arc<RunHost>,
    }

    impl ProviderRuns for MockRuns {
        fn source(&self) -> Source {
            Source::Claude
        }
        fn host(&self) -> &Arc<RunHost> {
            &self.host
        }
        fn add_to(&self, _: &mut DriverStatesBuilder) {}
        fn cli_name(&self) -> &'static str {
            "mock"
        }
        fn readable(&self, _: &droppen_provider::runs::RunRecord) -> bool {
            false
        }
        fn attach(
            self: Arc<Self>,
            _: droppen_provider::runs::RunRecord,
            _: droppen_provider::shim::Sidecars,
            _: droppen_provider::shim::Link,
            _: droppen_provider::gate::SessionLease,
        ) {
        }
        fn was_cut_short<'a>(&'a self, _: &'a droppen_provider::runs::RunRecord) -> BoxFuture<'a, bool> {
            Box::pin(std::future::ready(false))
        }
        fn last_write<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Option<Instant>> {
            Box::pin(std::future::ready(None))
        }
    }

    struct MockProvider {
        runs: Arc<MockRuns>,
        cfg: Arc<Mutex<HarnessConfig>>,
    }

    impl Provider for MockProvider {
        fn source(&self) -> Source {
            Source::Claude
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                questions: true,
                ..Capabilities::default()
            }
        }
        fn run(&self) -> &dyn Run {
            self
        }
        fn runs(&self) -> Arc<dyn ProviderRuns> {
            self.runs.clone()
        }
        fn models(&self) -> &dyn Models {
            self
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn continue_on_account<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, Result<PathBuf, ContinueError>> {
            Box::pin(std::future::ready(Err(ContinueError::Unsupported)))
        }
    }

    impl Models for MockProvider {
        fn list<'a>(&'a self, _: &'a ModelsQuery) -> BoxFuture<'a, Result<ModelList, String>> {
            Box::pin(std::future::ready(Err("none".into())))
        }
        fn ids<'a>(&'a self, _: Option<&'a str>) -> BoxFuture<'a, Vec<String>> {
            Box::pin(std::future::ready(Vec::new()))
        }
        fn accepted_efforts<'a>(&'a self, _: Option<&'a str>) -> BoxFuture<'a, Option<Vec<String>>> {
            Box::pin(std::future::ready(None))
        }
        fn permission_is_known(&self, _: &str) -> bool {
            false
        }
        fn account_model<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Option<String>> {
            Box::pin(std::future::ready(None))
        }
    }

    impl Run for MockProvider {
        fn send<'a>(&'a self, _: &'a SpawnGate, to: SendTo<'a>) -> BoxFuture<'a, Result<Sent, InjectError>> {
            let mut cfg = self.cfg.lock().unwrap();
            if cfg.fail_send {
                return Box::pin(std::future::ready(Err(InjectError::message("simulated send failure"))));
            }
            cfg.delivered_texts.push((to.id.to_string(), to.text.to_string()));
            Box::pin(std::future::ready(Ok(Sent { queued: false })))
        }
        fn start<'a>(&'a self, _: &'a SpawnGate, _: StartAt<'a>) -> BoxFuture<'a, Result<String, InjectError>> {
            Box::pin(std::future::ready(Err(InjectError::message("unsupported"))))
        }
        fn injection(&self, _: &str) -> Option<Injection> {
            None
        }
        fn resident(&self, id: &str) -> Option<ResidentState> {
            let cfg = self.cfg.lock().unwrap();
            cfg.residents.contains(id).then(|| ResidentState {
                phase: Phase::Idle,
                stalled: false,
                queued: 0,
                queued_messages: Vec::new(),
                background: 0,
                tasks: Vec::new(),
                awaiting_background: false,
                turn_started_at: None,
                question: None,
                permission: None,
                model: None,
            })
        }
        fn open_question(&self, id: &str) -> Option<String> {
            self.cfg.lock().unwrap().open_questions.get(id).cloned()
        }
        fn answer_question<'a>(
            &'a self,
            id: &'a str,
            request_id: &'a str,
            reply: Reply,
            by: Option<String>,
        ) -> BoxFuture<'a, AnswerOutcome> {
            let answer = match reply {
                Reply::Answer(a) => Some(a),
                Reply::Dismiss => None,
            };
            let store = self.runs.host.questions();
            let now = droppen_provider::questions::now_ms();
            let outcome = match store.resolve(
                request_id,
                &[QuestionState::Pending],
                QuestionState::Answered,
                answer.as_ref(),
                None,
                by.as_deref(),
                now,
            ) {
                Ok(q) => {
                    self.cfg.lock().unwrap().open_questions.remove(id);
                    AnswerOutcome::Delivered(q.wire())
                }
                Err(Some(q)) => AnswerOutcome::Resolved(q.wire()),
                Err(None) => AnswerOutcome::Unknown,
            };
            Box::pin(std::future::ready(outcome))
        }
        fn schedule_busy(&self, _: &str) -> Option<&'static str> {
            None
        }
        fn drop_queued<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, QueueOutcome> {
            Box::pin(std::future::ready(QueueOutcome::NoResident))
        }
        fn steer_queued<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, QueueOutcome> {
            Box::pin(std::future::ready(QueueOutcome::NoResident))
        }
        fn interrupt(&self, _: &str) -> bool {
            false
        }
        fn archive<'a>(&'a self, _: &'a SpawnGate, _: &'a str) -> BoxFuture<'a, Result<(), String>> {
            Box::pin(std::future::ready(Ok(())))
        }
    }

    thread_local! {
        static PENDING_CFG: std::cell::RefCell<Option<Arc<Mutex<HarnessConfig>>>> = const { std::cell::RefCell::new(None) };
    }

    fn register_mock(providers: &mut ProviderRegistry, registry: &Arc<InjectionRegistry>) {
        let cfg = PENDING_CFG
            .with(|slot| slot.borrow_mut().take())
            .expect("test harness cfg installed before Drive::new");
        providers.register(Arc::new(MockProvider {
            runs: Arc::new(MockRuns {
                host: registry.host.clone(),
            }),
            cfg,
        }));
    }

    fn build_harness() -> (tempfile::TempDir, DriveState, Arc<Mutex<HarnessConfig>>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Arc::new(Paths::new(dir.path().to_path_buf()));
        let instance = InstanceDirs::new(&paths.data_dir(), DEFAULT_INSTANCE).expect("instance");
        let auth = Auth::new(&paths, &instance, &AuthSettings::default(), false, Arc::new(NoNotify), Arc::new(NoAgents));
        let cfg = Arc::new(Mutex::new(HarnessConfig::default()));
        let mut sources = TranscriptSources::default();
        sources.register(Arc::new(MockReader { cfg: cfg.clone() }));
        PENDING_CFG.with(|slot| *slot.borrow_mut() = Some(cfg.clone()));
        let drive = Drive::new(
            paths.clone(),
            &instance,
            auth,
            sources,
            register_mock,
            Logins::stand_ins(),
            droppen_provider::testing::no_decoration(),
        );
        let global = feed::global::GlobalFeed::new();
        let state = DriveState {
            paths: drive.paths.clone(),
            drive,
            refresh: droppen_http::refresh::Refresh::new(global.clone()),
            global,
        };
        (dir, state, cfg)
    }

    fn add_session(dir: &Path, cfg: &Arc<Mutex<HarnessConfig>>, id: &str, scope: SessionScope, cwd: &str) {
        let file = dir.join(format!("{id}.jsonl"));
        std::fs::write(&file, format!("hello from {id}\n")).expect("write transcript");
        cfg.lock().unwrap().sessions.insert(
            id.to_string(),
            MockSession {
                id: id.to_string(),
                scope,
                cwd: cwd.to_string(),
                file,
            },
        );
    }

    fn caller_of(id: &str) -> AgentsCaller {
        AgentsCaller::parse(&format!("claude/{id}")).expect("caller")
    }

    #[tokio::test]
    async fn caller_validation_rejects_archived_deleted_chat_and_external_on_all_three_routes() {
        let (dir, state, cfg) = build_harness();
        let chat_cwd = chats_cwd(&state.paths).to_string_lossy().into_owned();

        add_session(dir.path(), &cfg, "owned-a", SessionScope::Owned, "/repo");
        add_session(dir.path(), &cfg, "owned-b", SessionScope::Owned, "/repo");
        add_session(dir.path(), &cfg, "archived-s", SessionScope::Owned, "/repo");
        add_session(dir.path(), &cfg, "chat-s", SessionScope::Owned, &chat_cwd);
        add_session(dir.path(), &cfg, "external-s", SessionScope::External, "/repo");
        {
            let mut guard = cfg.lock().unwrap();
            guard.archived.insert("archived-s".to_string());
            // Even with an idle resident still present, an archived caller must be rejected.
            guard.residents.insert("archived-s".to_string());
            guard.residents.insert("chat-s".to_string());
            // A freshly spawned non-archived resident without a transcript yet is admitted.
            guard.residents.insert("fresh-live".to_string());
        }

        for rejected in ["archived-s", "deleted-s", "chat-s", "external-s"] {
            let list_err = list_agents(State(state.clone()), caller_of(rejected), ApiQuery(ListAgentsQuery::default()))
                .await
                .expect_err(&format!("list should reject {rejected}"));
            assert_eq!(list_err.status(), StatusCode::FORBIDDEN, "list {rejected}");

            let send_err = send_agent_message(
                State(state.clone()),
                caller_of(rejected),
                ApiJson(SendAgentMessageBody {
                    target: "claude/owned-b".to_string(),
                    message: "hi".to_string(),
                }),
            )
            .await
            .expect_err(&format!("send should reject {rejected}"));
            assert_eq!(send_err.status(), StatusCode::FORBIDDEN, "send {rejected}");

            let read_err = read_agent_transcript(
                State(state.clone()),
                caller_of(rejected),
                ApiPath((Source::Claude, "owned-b".to_string())),
                ApiQuery(ReadAgentTranscriptQuery::default()),
            )
            .await
            .expect_err(&format!("read should reject {rejected}"));
            assert_eq!(read_err.status(), StatusCode::FORBIDDEN, "read {rejected}");
        }

        // Freshly spawned live resident without transcript on disk is admitted.
        let fresh_list = list_agents(State(state.clone()), caller_of("fresh-live"), ApiQuery(ListAgentsQuery::default()))
            .await
            .expect("fresh live resident admitted")
            .0;
        assert_eq!(fresh_list.caller.peer_ref, "claude/fresh-live");

        // Active owned non-chat session is admitted on all three routes.
        assert!(list_agents(State(state.clone()), caller_of("owned-a"), ApiQuery(ListAgentsQuery::default())).await.is_ok());
        assert!(read_agent_transcript(
            State(state.clone()),
            caller_of("owned-a"),
            ApiPath((Source::Claude, "owned-b".to_string())),
            ApiQuery(ReadAgentTranscriptQuery::default()),
        )
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn cwd_prefix_uses_path_boundaries_and_chat_targets_return_404() {
        let (dir, state, cfg) = build_harness();
        let chat_cwd = chats_cwd(&state.paths).to_string_lossy().into_owned();
        // Trailing slash variant tests `is_chat` path normalization vs `list_and_filter` string check.
        let chat_cwd_slash = format!("{chat_cwd}/");

        add_session(dir.path(), &cfg, "caller", SessionScope::Owned, "/workspace");
        add_session(dir.path(), &cfg, "in-repo", SessionScope::Owned, "/repo");
        add_session(dir.path(), &cfg, "in-sub", SessionScope::Owned, "/repo/sub");
        add_session(dir.path(), &cfg, "sibling", SessionScope::Owned, "/repo-2");
        add_session(dir.path(), &cfg, "chat-target", SessionScope::Owned, &chat_cwd_slash);

        let listed = list_agents(
            State(state.clone()),
            caller_of("caller"),
            ApiQuery(ListAgentsQuery {
                cwd: Some("/repo".to_string()),
                ..Default::default()
            }),
        )
        .await
        .expect("list")
        .0;
        let ids: HashSet<String> = listed.agents.into_iter().map(|p| p.id).collect();
        assert_eq!(ids, HashSet::from(["in-repo".to_string(), "in-sub".to_string()]));

        // Targeting a chat session returns 404 (not 409), matching `read_agent_transcript`.
        let send_err = send_agent_message(
            State(state.clone()),
            caller_of("caller"),
            ApiJson(SendAgentMessageBody {
                target: "claude/chat-target".to_string(),
                message: "hello".to_string(),
            }),
        )
        .await
        .expect_err("chat target rejected");
        assert_eq!(send_err.status(), StatusCode::NOT_FOUND);

        let read_err = read_agent_transcript(
            State(state.clone()),
            caller_of("caller"),
            ApiPath((Source::Claude, "chat-target".to_string())),
            ApiQuery(ReadAgentTranscriptQuery::default()),
        )
        .await
        .expect_err("chat read rejected");
        assert_eq!(read_err.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn failed_send_rolls_back_rate_quota_and_hop_state() {
        let (dir, state, cfg) = build_harness();
        add_session(dir.path(), &cfg, "sender", SessionScope::Owned, "/repo");
        add_session(dir.path(), &cfg, "target", SessionScope::Owned, "/repo");
        cfg.lock().unwrap().fail_send = true;

        for _ in 0..8 {
            let err = send_agent_message(
                State(state.clone()),
                caller_of("sender"),
                ApiJson(SendAgentMessageBody {
                    target: "claude/target".to_string(),
                    message: "retry me".to_string(),
                }),
            )
            .await
            .expect_err("simulated provider failure");
            assert_eq!(err.status(), StatusCode::CONFLICT, "must not trip 429 on failed sends");
            assert_eq!(state.drive.agents.hop_for("claude/target"), None);
        }

        cfg.lock().unwrap().fail_send = false;
        let ok = send_agent_message(
            State(state.clone()),
            caller_of("sender"),
            ApiJson(SendAgentMessageBody {
                target: "claude/target".to_string(),
                message: "now succeeds".to_string(),
            }),
        )
        .await
        .expect("delivery succeeds once provider recovers")
        .0;
        assert_eq!(ok.hop, 1);
        assert_eq!(state.drive.agents.hop_for("claude/target"), Some(1));
    }

    #[tokio::test]
    async fn human_answers_to_pending_and_expired_questions_reset_hops() {
        let (dir, state, cfg) = build_harness();
        add_session(dir.path(), &cfg, "sess-1", SessionScope::Owned, "/repo");

        let make_question = |prompt: &str| {
            serde_json::json!([{
                "question": prompt,
                "header": "Choice",
                "options": [{ "label": "Yes", "description": "Proceed" }],
                "multiSelect": false
            }])
        };

        // 1. Human answers a pending question via the composer (`POST /message`).
        let now = droppen_provider::questions::now_ms();
        state
            .drive
            .injections
            .host
            .questions()
            .open("req-composer", "sess-1", None, &make_question("Continue?"), now);
        cfg.lock()
            .unwrap()
            .open_questions
            .insert("sess-1".to_string(), "req-composer".to_string());
        state.drive.agents.record_hop_for_test(Source::Claude, "sess-1", 6, Instant::now());

        let resp = send_message(
            State(state.clone()),
            droppen_auth::MaybeIdentity(None),
            ApiPath((Source::Claude, "sess-1".to_string())),
            ApiJson(SendMessage {
                text: "Yes, go ahead".to_string(),
                model: Default::default(),
                effort: Default::default(),
                permission: Default::default(),
                account_id: None,
                images: Default::default(),
                files: Default::default(),
                client_message_id: Default::default(),
            }),
        )
        .await
        .expect("composer answer")
        .0;
        assert_eq!(resp.answered.0, Some(true));
        assert_eq!(state.drive.agents.hop_for("claude/sess-1"), None);

        // 2. Human answers a pending question via `POST .../questions/{requestId}`.
        state
            .drive
            .injections
            .host
            .questions()
            .open("req-pending", "sess-1", None, &make_question("Ship it?"), now);
        state.drive.agents.record_hop_for_test(Source::Claude, "sess-1", 5, Instant::now());
        let mut answers = HashMap::new();
        answers.insert("Ship it?".to_string(), AnswerValue::One("Yes".to_string()));
        answer_question(
            State(state.clone()),
            droppen_auth::MaybeIdentity(None),
            ApiPath(QuestionPath {
                source: Source::Claude,
                id: "sess-1".to_string(),
                request_id: "req-pending".to_string(),
            }),
            ApiJson(AnswerQuestion {
                answers,
                notes: Default::default(),
                response: Default::default(),
            }),
        )
        .await
        .expect("pending question answer");
        assert_eq!(state.drive.agents.hop_for("claude/sess-1"), None);

        // 3. Human answers an expired question (delivered as a new message).
        state
            .drive
            .injections
            .host
            .questions()
            .open("req-expired", "sess-1", None, &make_question("Retry?"), now);
        state
            .drive
            .injections
            .host
            .questions()
            .close("req-expired", QuestionState::Expired, now + 1);
        state.drive.agents.record_hop_for_test(Source::Claude, "sess-1", 6, Instant::now());
        let mut answers = HashMap::new();
        answers.insert("Retry?".to_string(), AnswerValue::One("Yes".to_string()));
        answer_question(
            State(state.clone()),
            droppen_auth::MaybeIdentity(None),
            ApiPath(QuestionPath {
                source: Source::Claude,
                id: "sess-1".to_string(),
                request_id: "req-expired".to_string(),
            }),
            ApiJson(AnswerQuestion {
                answers,
                notes: Default::default(),
                response: Default::default(),
            }),
        )
        .await
        .expect("expired question answer");
        assert_eq!(state.drive.agents.hop_for("claude/sess-1"), None);
    }
}
