//! Local session-to-session agent bus (`droppen mcp-agents`).
//!
//! Exposes a loopback-only HTTP surface (`/api/agents`, `/api/agents/send`,
//! `/api/agents/{source}/{id}/transcript`) authenticated by a stateless,
//! domain-separated per-session HMAC capability token (`rha1`). Every provider
//! (ACP, Claude Code, Codex) receives the same pure-Rust `droppen mcp-agents`
//! stdio MCP bridge so heterogeneous sessions on one device can discover each
//! other, read trimmed progress, and exchange `<cross-session-message>` turns
//! through Droppen's unified provider and transcript pipeline.

pub mod mcp;

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use gotcha::axum::extract::FromRequestParts;
use gotcha::axum::http::request::Parts;
use gotcha::axum::http::{header, HeaderMap, Method, StatusCode};
use gotcha::{Json, State};
use serde::{Deserialize, Serialize};

use droppen_base::process_env::{AGENTS_TOKEN_ENV, AGENTS_URL_ENV};
use droppen_http::error::ApiError;
use droppen_http::extract::{ApiJson, ApiPath, ApiQuery};
use droppen_http::identity::AgentsGrant;
use droppen_instance::instance::InstanceDirs;
use droppen_instance::paths::Paths;
use droppen_provider::{SendOptions, StdioMcpServer};
use droppen_sessions::attention::seen::key_of;
use droppen_sessions::util::{ItemKind, Role};
use droppen_sessions::{chats_cwd, file_signature, ScopeFilter, SessionScope, SessionSummary, Source};

use crate::feed;
use crate::routes::DriveState;

/// List sessions visible on the local agent bus.
pub const AGENTS_LIST_PATH: &str = "/api/agents";
/// Deliver a cross-session message to another session.
pub const AGENTS_SEND_PATH: &str = "/api/agents/send";
/// Read a trimmed transcript summary of another session.
pub const AGENTS_TRANSCRIPT_PATH: &str = "/api/agents/{source}/{id}/transcript";

/// Domain-separated token prefix (distinct from `rh-notify`'s `rhn1`).
const TOKEN_PREFIX: &str = "rha1";
/// HMAC domain separator (distinct from `rh-notify:v1`).
const TOKEN_DOMAIN: &str = "droppen-agents:v1";
/// Name under which the injected stdio MCP server is registered.
pub const MCP_SERVER_NAME: &str = "droppen-agents";

/// Maximum size of one cross-session message body in bytes.
const MAX_MESSAGE_BYTES: usize = 16 * 1024;
/// Maximum messages one caller may send to the same target inside [`RATE_WINDOW`].
const RATE_LIMIT_MAX: usize = 6;
/// Sliding window for per-pair rate limiting.
const RATE_WINDOW: Duration = Duration::from_secs(60);
/// Maximum consecutive cross-session hops before human intervention is required.
const MAX_HOPS: u32 = 6;
/// Idle timeout after which consecutive hop counts reset automatically.
const HOP_RESET_AFTER: Duration = Duration::from_secs(10 * 60);
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

// ------------------------------------------------------------------ caller & tokens

/// The session proved by a verified `rha1` bearer token.
///
/// Carried in request extensions inside [`AgentsGrant`] by `droppen_auth::guard`
/// (loopback only, never over the cloud bridge) and never interchangeable with
/// `AuthIdentity` or `NotifyCaller`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentsCaller {
    source: Source,
    id: String,
}

impl AgentsCaller {
    /// Which provider owns the calling session.
    pub fn source(&self) -> Source {
        self.source
    }

    /// Calling session id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Canonical `<source>/<id>` reference.
    pub fn key(&self) -> String {
        key_of(self.source, &self.id)
    }

    /// Parse `<source>/<id>` back into a caller.
    pub fn parse(key: &str) -> Option<Self> {
        let (source, id) = key.split_once('/')?;
        let source = Source::parse(source)?;
        (!id.is_empty()).then(|| Self {
            source,
            id: id.to_string(),
        })
    }
}

impl<S: Send + Sync> FromRequestParts<S> for AgentsCaller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AgentsGrant>()
            .and_then(|grant| grant.get::<AgentsCaller>())
            .cloned()
            .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized"))
    }
}

/// Whether `method` + `path` targets the local agent-bus routes.
pub fn is_agents_request(method: &Method, path: &str) -> bool {
    if method == Method::GET && path == AGENTS_LIST_PATH {
        return true;
    }
    if method == Method::POST && path == AGENTS_SEND_PATH {
        return true;
    }
    if method == Method::GET {
        if let Some(rest) = path.strip_prefix("/api/agents/") {
            if let Some(mid) = rest.strip_suffix("/transcript") {
                let mut parts = mid.split('/');
                let source = parts.next();
                let id = parts.next();
                if source.is_some_and(|s| Source::parse(s).is_some())
                    && id.is_some_and(|id| !id.is_empty())
                    && parts.next().is_none()
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Mint a stateless `rha1` capability token for session `(source, id)`.
pub fn mint_token_with_secret(secret: &[u8], instance: &str, source: Source, id: &str) -> String {
    let key = key_of(source, id);
    droppen_notify::notify::token::mint_scoped(secret, TOKEN_PREFIX, TOKEN_DOMAIN, instance, &key)
}

/// Verify an `rha1` capability token against `secret` and `instance`.
pub fn verify_token_with_secret(secret: &[u8], instance: &str, token: &str) -> Option<AgentsCaller> {
    let key = droppen_notify::notify::token::verify_scoped(secret, TOKEN_PREFIX, TOKEN_DOMAIN, instance, token)?;
    AgentsCaller::parse(&key)
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

/// Verify the `Authorization: Bearer rha1...` header on a request without
/// minting a secret if none exists yet.
pub async fn caller(paths: &Paths, instance: &str, headers: &HeaderMap) -> Option<AgentsCaller> {
    let offered = bearer(headers)?;
    let secret = droppen_notify::notify::store::read_secret(paths).await?;
    verify_token_with_secret(&secret, instance, offered)
}

/// Build the `droppen-agents` stdio MCP server descriptor for session `(source, id)`.
///
/// Returns `None` if the server has not written its loopback port file (for
/// example in lock-less unit tests) or if the signing secret cannot be read.
pub async fn mcp_server_for(paths: &Paths, instance: &InstanceDirs, source: Source, id: &str) -> Option<StdioMcpServer> {
    let port: u16 = std::fs::read_to_string(instance.port_file()).ok()?.trim().parse().ok()?;
    let secret = match droppen_notify::notify::store::secret(paths).await {
        Ok(secret) => secret,
        Err(error) => {
            tracing::warn!(%error, "agents: no usable token secret; skipping droppen-agents MCP server");
            return None;
        }
    };
    let token = mint_token_with_secret(&secret, instance.name(), source, id);
    let binary = std::env::current_exe().ok()?;
    Some(StdioMcpServer {
        name: MCP_SERVER_NAME.to_string(),
        command: binary,
        args: vec!["mcp-agents".to_string()],
        env: vec![
            (AGENTS_URL_ENV.to_string(), format!("http://127.0.0.1:{port}")),
            (AGENTS_TOKEN_ENV.to_string(), token),
        ],
    })
}

// ------------------------------------------------------------------ anti-loop state

/// In-memory anti-loop and rate-limit state for the local agent bus.
#[derive(Default)]
pub struct AgentsBus {
    rate_windows: Mutex<HashMap<(String, String), VecDeque<Instant>>>,
    hop_state: Mutex<HashMap<String, (u32, Instant)>>,
}

/// Reason a cross-session send was throttled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThrottleError {
    /// Pair exceeded [`RATE_LIMIT_MAX`] messages within [`RATE_WINDOW`].
    RateLimited,
    /// Consecutive cross-session hops exceeded [`MAX_HOPS`].
    HopLimitExceeded,
}

impl AgentsBus {
    /// Reset the consecutive cross-session hop counter when a human sends a
    /// message directly to session `(source, id)`.
    pub fn reset_hops(&self, source: Source, id: &str) {
        let key = key_of(source, id);
        self.hop_state.lock().unwrap().remove(&key);
    }

    /// Check and record a send from `from_key` to `to_key`, returning the hop
    /// number stamped on the outgoing envelope.
    fn check_and_record(&self, from_key: &str, to_key: &str, now: Instant) -> Result<u32, ThrottleError> {
        let next_hop = {
            let hops = self.hop_state.lock().unwrap();
            let caller_hop = hops
                .get(from_key)
                .filter(|(_, at)| now.duration_since(*at) <= HOP_RESET_AFTER)
                .map(|(h, _)| *h)
                .unwrap_or(0);
            caller_hop.saturating_add(1)
        };
        if next_hop > MAX_HOPS {
            return Err(ThrottleError::HopLimitExceeded);
        }

        {
            let mut windows = self.rate_windows.lock().unwrap();
            let slot = windows.entry((from_key.to_string(), to_key.to_string())).or_default();
            while let Some(&front) = slot.front() {
                if now.duration_since(front) >= RATE_WINDOW {
                    slot.pop_front();
                } else {
                    break;
                }
            }
            if slot.len() >= RATE_LIMIT_MAX {
                return Err(ThrottleError::RateLimited);
            }
            slot.push_back(now);
        }

        self.hop_state.lock().unwrap().insert(to_key.to_string(), (next_hop, now));
        Ok(next_hop)
    }
}

// ------------------------------------------------------------------ wire types

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
    /// Live status (`active` / `idle`).
    pub status: droppen_sessions::SessionStatus,
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
    /// Other owned sessions visible on this Droppen instance.
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
    pub status: droppen_sessions::SessionStatus,
    /// Fine-grained activity.
    pub activity: droppen_sessions::SessionActivity,
    /// Whether the session is blocked waiting on a human permission approval.
    pub pending_permission: bool,
    /// Whether the session is blocked waiting on a human question answer.
    pub pending_question: bool,
    /// Recent trimmed transcript items in chronological order.
    pub items: Vec<AgentTranscriptItem>,
}

// ------------------------------------------------------------------ helpers

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
        source: caller.source,
        id: caller.id.clone(),
        acp_agent: None,
        agent_name: None,
        title: caller.key(),
        cwd: None,
        status: droppen_sessions::SessionStatus::Running,
        activity: droppen_sessions::SessionActivity::Working,
        can_receive: true,
        updated_at: String::new(),
    }
}

fn escape_attr(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' | '\n' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

/// Format a cross-session envelope matching Droppen's `<cross-session-message>` parser.
pub fn wrap_cross_session_message(from_ref: &str, from_name: &str, hop: u32, message: &str) -> String {
    let from = escape_attr(from_ref);
    let name = escape_attr(from_name);
    format!(
        "<cross-session-message from=\"{from}\" from-name=\"{name}\" hop=\"{hop}\">\n\n{}\n</cross-session-message>",
        message.trim()
    )
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
            .unwrap_or_else(|| caller.source.as_str());
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
            .find(|s| s.source == parsed.source && s.id == parsed.id)
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

// ------------------------------------------------------------------ handlers

/// `GET /api/agents` — list owned sessions visible on the local agent bus.
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
        .find(|s| s.source == caller.source && s.id == caller.id)
        .map(|s| to_peer(drive, s))
        .unwrap_or_else(|| fallback_self_peer(&caller));

    let mut peers: Vec<AgentPeer> = sessions
        .iter()
        .filter(|s| !(s.source == caller.source && s.id == caller.id))
        .filter(|s| !is_chat(&drive.paths, s))
        .filter(|s| query.source.is_none_or(|wanted| s.source == wanted))
        .filter(|s| {
            query
                .cwd
                .as_deref()
                .filter(|c| !c.is_empty())
                .is_none_or(|prefix| s.cwd.0.as_deref().is_some_and(|cwd| cwd.starts_with(prefix)))
        })
        .filter(|s| !active_only || s.status != droppen_sessions::SessionStatus::Idle)
        .map(|s| to_peer(drive, s))
        .collect();

    // Active sessions first, preserving recency ordering within each group.
    peers.sort_by_key(|p| p.status == droppen_sessions::SessionStatus::Idle);
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
    if target.source == caller.source && target.id == caller.id {
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
                format!(
                    "cross-session hop limit ({MAX_HOPS}) reached; pause and let the user intervene"
                ),
            ),
        })?;

    let caller_summary = sessions
        .iter()
        .find(|s| s.source == caller.source && s.id == caller.id);
    let from_name = sender_display_name(&caller, caller_summary);
    let wrapped = wrap_cross_session_message(&caller_ref, &from_name, hop, message);

    // Explicitly default SendOptions: never override permission/model/account,
    // and never route through `answer_question` (peer messages must never
    // approve permissions or answer user questions).
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

    // Cap total response text budget from newest backwards.
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

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

    #[test]
    fn agents_token_is_domain_separated_from_notify_token() {
        let agents = mint_token_with_secret(SECRET, "default", Source::Acp, "sess-1");
        assert!(agents.starts_with("rha1."));
        let verified = verify_token_with_secret(SECRET, "default", &agents).expect("valid agents token");
        assert_eq!(verified.source(), Source::Acp);
        assert_eq!(verified.id(), "sess-1");
        assert_eq!(verify_token_with_secret(SECRET, "other", &agents), None);

        // A notify token must never verify as an agents token and vice versa.
        let notify = droppen_notify::notify::token::mint(SECRET, "default", "acp/sess-1");
        assert_eq!(verify_token_with_secret(SECRET, "default", &notify), None);
        assert_eq!(droppen_notify::notify::token::verify(SECRET, "default", &agents), None);
    }

    #[test]
    fn route_matcher_covers_only_agents_paths() {
        assert!(is_agents_request(&Method::GET, "/api/agents"));
        assert!(is_agents_request(&Method::POST, "/api/agents/send"));
        assert!(is_agents_request(&Method::GET, "/api/agents/claude/abc-123/transcript"));
        assert!(is_agents_request(&Method::GET, "/api/agents/acp/9711/transcript"));
        assert!(is_agents_request(&Method::GET, "/api/agents/codex/t1/transcript"));

        assert!(!is_agents_request(&Method::POST, "/api/agents"));
        assert!(!is_agents_request(&Method::GET, "/api/agents/send"));
        assert!(!is_agents_request(&Method::GET, "/api/agents/unknown/abc/transcript"));
        assert!(!is_agents_request(&Method::GET, "/api/agents/claude//transcript"));
        assert!(!is_agents_request(&Method::GET, "/api/sessions"));
    }

    #[test]
    fn cross_session_envelope_escapes_attributes_and_preserves_body() {
        let wrapped = wrap_cross_session_message("acp/s1", "agy: fix \"quotes\" & <tags>\nline2", 2, "  hello peer  ");
        assert!(wrapped.starts_with("<cross-session-message from=\"acp/s1\" from-name=\"agy: fix &quot;quotes&quot; &amp; &lt;tags&gt; line2\" hop=\"2\">\n\n"));
        assert!(wrapped.ends_with("\nhello peer\n</cross-session-message>"));
        assert_eq!(droppen_sessions::util::title_line(&wrapped), "hello peer");
    }

    #[test]
    fn anti_loop_enforces_rate_limit_and_hop_ceiling_and_resets_on_human_turn() {
        let bus = AgentsBus::default();
        let start = Instant::now();

        // Ping-pong A -> B -> A -> B -> A -> B (6 hops) succeeds, 7th fails.
        assert_eq!(bus.check_and_record("acp/a", "claude/b", start), Ok(1));
        assert_eq!(bus.check_and_record("claude/b", "acp/a", start), Ok(2));
        assert_eq!(bus.check_and_record("acp/a", "claude/b", start), Ok(3));
        assert_eq!(bus.check_and_record("claude/b", "acp/a", start), Ok(4));
        assert_eq!(bus.check_and_record("acp/a", "claude/b", start), Ok(5));
        assert_eq!(bus.check_and_record("claude/b", "acp/a", start), Ok(6));
        assert_eq!(bus.check_and_record("acp/a", "claude/b", start), Err(ThrottleError::HopLimitExceeded));

        // Human intervention resets hop count for acp/a.
        bus.reset_hops(Source::Acp, "a");
        assert_eq!(bus.check_and_record("acp/a", "claude/b", start), Ok(1));

        // Rate limit per pair: 6 within 60s allowed across a fresh pair, 7th refused.
        let bus2 = AgentsBus::default();
        for i in 1..=RATE_LIMIT_MAX {
            assert_eq!(bus2.check_and_record("acp/x", "claude/y", start), Ok(1), "slot {i}");
        }
        assert_eq!(bus2.check_and_record("acp/x", "claude/y", start), Err(ThrottleError::RateLimited));
        // Sliding window expires after 60s.
        assert_eq!(
            bus2.check_and_record("acp/x", "claude/y", start + RATE_WINDOW + Duration::from_secs(1)),
            Ok(1)
        );
    }
}
