//! Claude Code's provider: its resident driver, its model list, its session
//! reader, the config roots its spawns run against, and the recording each
//! run leaves.

pub mod claude_roots;
pub mod models;
pub mod record;
pub(crate) mod resident;
pub mod sessions;
