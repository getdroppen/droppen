//! Codex's session reader: its rollouts, discovery, tool cards, and the
//! claims that keep one instance's threads from being listed by another, read
//! through the [`droppen_sessions::Transcripts`] port.

pub mod codex;
pub mod codex_claims;
pub mod codex_tools;
