//! The provider implementations that are still the server's own: Claude
//! Code's and Codex's, each until it moves into a crate of its own. What they
//! stand on is the `droppen_provider` crate; the ACP agents' provider is the
//! `droppen_provider_acp` crate.

pub mod claude_code;
pub mod codex;
#[cfg(test)]
mod with_providers;
