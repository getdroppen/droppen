//! The provider base's tests that need real providers (`with_providers`).
//! Each provider is a crate of its own on the `droppen_provider` crate:
//! Claude Code's `droppen_provider_claude_code`, Codex's
//! `droppen_provider_codex` and the ACP agents' `droppen_provider_acp`.

#[cfg(test)]
pub(crate) mod with_providers;
