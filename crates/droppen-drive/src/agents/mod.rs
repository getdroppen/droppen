//! Local session-to-session agent bus (`droppen mcp-agents`).
//!
//! Sessions driven by different providers on the same machine — ACP agents such
//! as Antigravity, Claude Code, and Codex — cannot use one provider's private
//! peer socket to talk to another's. Instead, every session is given the same
//! pure-Rust stdio MCP bridge (`droppen mcp-agents`), which speaks to this
//! module over loopback with a stateless, domain-separated capability token
//! (`rha1`). Every peer message goes through the ordinary provider delivery
//! path in a `<cross-session-message>` envelope so it appears in the target's
//! transcript and obeys the same turn queue and stop controls as user input.

pub mod bus;
pub mod mcp;
pub mod routes;
pub mod token;

pub use bus::{wrap_cross_session_message, AgentsBus, ThrottleError};
pub use routes::{
    list_agents, read_agent_transcript, send_agent_message, AgentPeer, AgentTranscriptItem,
    ListAgentsQuery, ListAgentsResponse, ReadAgentTranscriptQuery, ReadAgentTranscriptResponse,
    SendAgentMessageBody, SendAgentMessageResponse, AGENTS_LIST_PATH, AGENTS_SEND_PATH,
    AGENTS_TRANSCRIPT_PATH,
};
pub use token::{
    caller, is_agents_request, mcp_server_for, mint_token_with_secret, verify_token_with_secret,
    AgentsCaller, MCP_SERVER_NAME,
};
