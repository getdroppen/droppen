//! What some CLIs keep beside a session's transcript: the transcripts and
//! metas of the subagents it spawned, and its task checklist.
//!
//! A CLI that keeps them says so through [`super::Transcripts::subagents`] and
//! [`super::Transcripts::checklists`]; the session routes ask those and name no
//! CLI. A CLI that keeps neither answers `None`, and the routes answer 404.

use std::path::{Path, PathBuf};

use futures_util::future::BoxFuture;
use gotcha::Schematic;
use serde::{Deserialize, Serialize};

use super::util::TranscriptItem;
use super::Source;
use droppen_instance::paths::Paths;

/// The subagents a CLI's sessions spawn, each with a transcript of its own.
pub trait Subagents: Send + Sync {
    /// Every agent that ever wrote under the session whose transcript is
    /// `session_file`, newest work first.
    fn agents<'a>(&'a self, session_file: &'a Path) -> BoxFuture<'a, Vec<AgentEntry>>;

    /// The transcript of agent `agent_id` of that session, wherever under the
    /// session it was filed; `None` when there is none. The caller has already
    /// checked that `agent_id` can name a file.
    fn agent_file<'a>(&'a self, session_file: &'a Path, agent_id: &'a str) -> BoxFuture<'a, Option<PathBuf>>;

    /// The agent metas in `dir`, the directory an agent's transcript is in.
    fn agent_metas<'a>(&'a self, dir: &'a Path) -> BoxFuture<'a, Vec<SubagentMeta>>;

    /// The items of the agent transcript at `file_path`.
    fn agent_items<'a>(&'a self, paths: &'a Paths, file_path: &'a Path) -> BoxFuture<'a, std::io::Result<Vec<TranscriptItem>>>;
}

/// Path of `GET /api/sessions/{source}/{id}/agents/{agentId}`.
///
/// A struct rather than a tuple because gotcha only documents one- and
/// two-segment tuple paths (`gotcha_core-0.4.1/src/parameter.rs:43,60`); the
/// generic `Path<T>` impl reads a struct's fields instead, which is the only
/// way to document three.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AgentPath {
    /// Which CLI the parent session belongs to.
    pub source: Source,
    /// The parent session's id.
    pub id: String,
    /// The subagent's id.
    pub agent_id: String,
}

/// Whether an agent id can name a file next to a transcript.
pub(crate) fn is_agent_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

/// The task checklists a CLI keeps for its sessions.
pub trait Checklists: Send + Sync {
    /// Session `id`'s checklist, or `None` when there is no such session or
    /// its id cannot name a checklist directory.
    fn read<'a>(&'a self, paths: &'a Paths, id: &'a str) -> BoxFuture<'a, Option<Checklist>>;
}

/// Metadata a subagent transcript writes next to the session file.
#[derive(Debug, Clone)]
pub struct SubagentMeta {
    /// The subagent's id.
    pub agent_id: String,
    /// The `tool_use` id of the Task call that spawned it.
    pub tool_use_id: Option<String>,
    /// What the Task call asked for.
    pub description: Option<String>,
    /// Which agent type answered.
    pub agent_type: Option<String>,
    /// How deep the spawn chain is: `1` for a child of the session itself, `2`
    /// for a child of a child.
    ///
    /// The field that explains an unattachable `toolUseId`: at depth 2 and below
    /// the id names a `tool_use` in the *parent agent's* transcript, not in the
    /// session's, so looking for it in the main file finds nothing and the whole
    /// subtree goes missing. One real session (`b16232e3`) had eleven agents and
    /// 2.5 MB of work invisible for exactly that reason.
    pub spawn_depth: Option<u32>,
    /// The agent that spawned it, for anything deeper than the session.
    pub parent_agent_id: Option<String>,
    /// Which model the child ran on, when the meta says.
    pub model: Option<String>,
}

/// One agent found on disk under a session, meta or no meta.
#[derive(Debug, Clone, Default)]
pub struct AgentEntry {
    /// The agent's id, which is what `/agents/{agentId}` takes.
    pub agent_id: String,
    /// Which agent type answered, from the meta.
    pub agent_type: Option<String>,
    /// What the spawning call asked for, from the meta.
    pub description: Option<String>,
    /// The `tool_use` id the meta recorded, if any.
    pub tool_use_id: Option<String>,
    /// The agent that spawned it, for anything deeper than the session.
    pub parent_agent_id: Option<String>,
    /// How deep the spawn chain is; `1` is a direct child of the session.
    pub spawn_depth: Option<u32>,
    /// Which model it ran on.
    pub model: Option<String>,
    /// Whether a transcript file exists beside the meta.
    pub has_transcript: bool,
    /// Where that transcript is, when it exists.
    pub transcript_path: Option<PathBuf>,
    /// Size of that transcript, in bytes.
    pub bytes: u64,
    /// When it was last written, by the filesystem's clock.
    pub updated_at: Option<String>,
}

impl AgentEntry {
    /// An agent found under its id, before anything is known about it.
    pub fn new(agent_id: &str) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            ..Self::default()
        }
    }

    /// Take what the agent's meta says about it.
    pub fn absorb(&mut self, meta: SubagentMeta) {
        self.agent_type = meta.agent_type;
        self.description = meta.description;
        self.tool_use_id = meta.tool_use_id;
        self.parent_agent_id = meta.parent_agent_id;
        self.spawn_depth = meta.spawn_depth;
        self.model = meta.model;
    }
}

/// One task in a session's checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct ChecklistItem {
    /// The task's own id, as the CLI numbered it.
    pub id: String,
    /// What the task is; the sidecar calls this field `subject`.
    pub title: String,
    /// Where it got to: `pending`, `in_progress`, `completed`. Carried as free
    /// text rather than an enum because the vocabulary is the CLI's own — a word
    /// this server has not seen should reach the UI, not fail the request.
    pub status: String,
}

/// A session's checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct Checklist {
    /// The tasks, in the order the CLI numbered them.
    pub items: Vec<ChecklistItem>,
    /// How many tasks there are.
    pub total: usize,
    /// How many are finished.
    pub done: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_id_is_a_plain_file_name() {
        assert!(is_agent_id("agent_1-2"));
        assert!(!is_agent_id("../escape"));
        assert!(!is_agent_id(""));
        assert!(!is_agent_id(&"a".repeat(65)));
    }
}
