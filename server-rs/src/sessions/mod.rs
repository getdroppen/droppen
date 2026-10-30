//! Each CLI's own session reader, until it moves into its provider: Claude
//! Code's, Codex's and the ACP agents' transcripts, discovery and ownership,
//! read through the [`droppen_sessions::Transcripts`] port.

pub mod acp_transcript;
pub mod codex;
pub mod codex_claims;
pub mod codex_tools;
