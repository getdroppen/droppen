//! Stateless loopback capability tokens (`rha1`) for the local agent bus.
//!
//! Structured like `droppen_notify::notify::token`, with a distinct prefix
//! (`rha1`) and HMAC domain (`droppen-agents:v1`) so a notify token and an
//! agent-bus token can never verify as each other even though both derive from
//! the same instance secret. Being stateless lets a long-lived session survive
//! a server restart without losing its peer identity.

use gotcha::axum::extract::FromRequestParts;
use gotcha::axum::http::request::Parts;
use gotcha::axum::http::{header, HeaderMap, Method, StatusCode};

use droppen_base::process_env::{AGENTS_TOKEN_ENV, AGENTS_URL_ENV};
use droppen_http::error::ApiError;
use droppen_http::identity::AgentsGrant;
use droppen_instance::instance::InstanceDirs;
use droppen_instance::paths::Paths;
use droppen_provider::StdioMcpServer;
use droppen_sessions::attention::seen::key_of;
use droppen_sessions::Source;

use super::routes::{AGENTS_LIST_PATH, AGENTS_SEND_PATH};

/// Domain-separated token prefix, distinct from `rh-notify`'s `rhn1`.
const TOKEN_PREFIX: &str = "rha1";
/// HMAC domain separator, distinct from `rh-notify:v1`.
const TOKEN_DOMAIN: &str = "droppen-agents:v1";
/// Name under which the injected stdio MCP server is registered in every host.
pub const MCP_SERVER_NAME: &str = "droppen-agents";

/// The session proved by a verified `rha1` bearer token.
///
/// Carried in request extensions inside [`AgentsGrant`] by `droppen_auth::guard`
/// on direct loopback requests only (never over the cloud bridge) and never
/// interchangeable with `AuthIdentity` or `NotifyGrant`.
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

    /// Canonical `<source>/<id>` address.
    pub fn key(&self) -> String {
        key_of(self.source, &self.id)
    }

    /// Parse `<source>/<id>` into a caller address.
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

/// Whether `method` and `path` target one of the local agent-bus routes.
///
/// Checked by `droppen_auth::guard` before reading the secret file so ordinary
/// API requests pay no disk lookup.
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

/// Mint a stateless `rha1` capability token for `(source, id)` on `instance`.
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
/// creating a signing secret if none exists yet.
pub async fn caller(paths: &Paths, instance: &str, headers: &HeaderMap) -> Option<AgentsCaller> {
    let offered = bearer(headers)?;
    let secret = droppen_notify::notify::store::read_secret(paths).await?;
    verify_token_with_secret(&secret, instance, offered)
}

/// Build the `droppen-agents` stdio MCP server descriptor for `(source, id)`.
///
/// Returns `None` when the server has not written its loopback port file (for
/// instance in lock-less unit tests) or when no signing secret can be obtained.
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

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

    #[test]
    fn an_agents_token_never_verifies_as_a_notify_token_or_for_another_instance() {
        let agents = mint_token_with_secret(SECRET, "default", Source::Acp, "sess-1");
        assert!(agents.starts_with("rha1."));
        let verified = verify_token_with_secret(SECRET, "default", &agents).expect("valid agents token");
        assert_eq!(verified.source(), Source::Acp);
        assert_eq!(verified.id(), "sess-1");
        assert_eq!(verify_token_with_secret(SECRET, "other", &agents), None);

        let notify = droppen_notify::notify::token::mint(SECRET, "default", "acp/sess-1");
        assert_eq!(verify_token_with_secret(SECRET, "default", &notify), None);
        assert_eq!(droppen_notify::notify::token::verify(SECRET, "default", &agents), None);
    }

    #[test]
    fn the_route_matcher_admits_only_the_three_agent_bus_paths() {
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
}
