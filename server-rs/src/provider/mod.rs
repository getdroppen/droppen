//! The provider implementation that is still the server's own: Claude Code's,
//! until it moves into a crate of its own. What it stands on is the
//! `droppen_provider` crate; the ACP agents' provider is the
//! `droppen_provider_acp` crate, and Codex's the `droppen_provider_codex`
//! crate.

pub mod claude_code;
#[cfg(test)]
mod with_providers;
