//! Archiving a session out of the list: what an archive answers when it
//! cannot put one away. Each CLI's reader archives its own sessions.

/// The `codex archive` call failed, with the CLI's own words where it had any.
#[derive(Debug, Clone, thiserror::Error)]
#[error("codex archive failed: {0}")]
pub struct ArchiveError(pub String);
