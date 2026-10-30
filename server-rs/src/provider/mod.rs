//! The provider implementations that are still the server's own: Claude
//! Code's, Codex's and the ACP agents', each until it moves into a crate of
//! its own. What they stand on is the `droppen_provider` crate.

pub mod acp;
pub mod claude_code;
pub mod codex;
#[cfg(test)]
mod with_providers;
