//! The subagents a session delegated work to, as a tree under it.
//!
//! Each CLI keeps who spawned whom in its own way, so each fills the tree of
//! its own sessions ([`super::Transcripts::children`]). A child is either a
//! session of its own, opened by its id ([`SessionChildKind::Thread`]), or a
//! transcript inside its parent, opened through it ([`SessionChildKind::Agent`]).
//!
//! Only delegated work is a child. A forked or continued conversation is a
//! session in its own right with no parent link, and stays a top-level row.
//!
//! The tree is an overlay ([`apply`]) like `unread`, computed each time the
//! list is, because a child writes to a file of its own: the parent's
//! transcript can sit still while its children change.

use std::collections::BTreeMap;

use gotcha::Schematic;
use serde::{Deserialize, Serialize};

use super::attention::seen;
use super::{SessionSummary, Source, TranscriptSources};
use droppen_http::openapi_ext::Nullable;
use droppen_instance::paths::Paths;

/// How a child is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum SessionChildKind {
    /// A session of its own: open it by its `id`, under the parent's source.
    Thread,
    /// A transcript inside its parent session: open it as the parent's agent
    /// `id` (`/api/sessions/{source}/{parent}/agents/{id}`).
    Agent,
}

/// Where a child is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum SessionChildStatus {
    /// Still at work.
    Working,
    /// Stopped and waiting on someone.
    Waiting,
    /// Finished.
    Done,
    /// Ended in an error, or was cut short.
    Failed,
}

/// One delegated subagent of a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SessionChild {
    /// Its id: a thread id, or an agent id within the parent; see `kind`.
    pub id: String,
    /// How it is opened.
    pub kind: SessionChildKind,
    /// What to call it: the name the runtime gave it, or what it was asked to do.
    pub name: String,
    /// What it was spawned as (an agent type or role), when the runtime says.
    pub role: Nullable<String>,
    /// Where it is.
    pub status: SessionChildStatus,
    /// When it last wrote, ISO-8601.
    pub updated_at: String,
    /// Whether it has output nobody has looked at: it wrote after the reader
    /// last opened it or anything above it — its parent's conversation shows
    /// what it delegated — whichever was latest.
    pub unread: bool,
    /// The subagents it delegated to in turn.
    pub children: Vec<SessionChild>,
}

/// Fill in `children` on every row, while the `subagentTree` pref is on.
pub async fn apply(paths: &Paths, transcripts: &TranscriptSources, sessions: &mut [SessionSummary]) {
    if sessions.is_empty() {
        return;
    }
    // Opt-in (`subagentTree` in the prefs): until it is on, nothing is looked
    // for, and every row goes out with no children, as before the tree.
    if crate::prefs::PrefsStore::of(paths).read().await.subagent_tree != crate::prefs::SubagentTree::On {
        return;
    }
    let marks = seen::read(paths).await;
    // Each CLI gets all of its rows at once, so what it reads for every one of
    // them (Codex's spawn edges) is read once per listing.
    let mut by_source: BTreeMap<Source, Vec<&mut SessionSummary>> = BTreeMap::new();
    for session in sessions.iter_mut() {
        by_source.entry(session.source).or_default().push(session);
    }
    for (source, sessions) in by_source {
        if let Some(reader) = transcripts.get(source) {
            reader.children(paths, sessions, &marks).await;
        }
    }
}

/// The reader's read marks, as [`seen::read`] returns them.
pub(super) trait ReadMarksExt {
    /// The reader's mark on `id`, if they ever opened it.
    fn read_at(&self, source: Source, id: &str) -> Option<&str>;
}

impl ReadMarksExt for BTreeMap<String, String> {
    fn read_at(&self, source: Source, id: &str) -> Option<&str> {
        self.get(&seen::key_of(source, id)).map(String::as_str)
    }
}

/// The id a Claude subagent's read mark is kept under: within its session.
pub fn agent_seen_id(session_id: &str, agent_id: &str) -> String {
    format!("{session_id}/agents/{agent_id}")
}

impl SessionChildStatus {
    /// Whether a child at this status, which last wrote at `updated_at`, has
    /// output its reader has not seen, given the later of its own mark and its
    /// parent's.
    pub(super) fn is_unread(self, updated_at: &str, own: Option<&str>, parent: Option<&str>) -> bool {
        if self == SessionChildStatus::Working {
            return false;
        }
        match own.max(parent) {
            Some(read) => updated_at > read,
            None => true,
        }
    }
}

/// Children put in the order they were spawned.
pub(super) trait SpawnOrderExt {
    /// Spawn order, as near as the files tell it: oldest write first, then id.
    fn sort_by_spawn(&mut self);
}

impl SpawnOrderExt for [SessionChild] {
    fn sort_by_spawn(&mut self) {
        self.sort_by(|a, b| a.updated_at.cmp(&b.updated_at).then_with(|| a.id.cmp(&b.id)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_is_unread_until_it_or_its_parent_is_opened_after_it_wrote() {
        let wrote = "2026-10-01T10:00:00Z";
        assert!(SessionChildStatus::Done.is_unread(wrote, None, None), "never opened");
        assert!(
            !SessionChildStatus::Done.is_unread(wrote, None, Some("2026-10-01T11:00:00Z")),
            "the parent was read since"
        );
        assert!(
            !SessionChildStatus::Done.is_unread(wrote, Some("2026-10-01T10:30:00Z"), None),
            "it was read since"
        );
        assert!(SessionChildStatus::Failed.is_unread(wrote, Some("2026-10-01T09:00:00Z"), Some("2026-10-01T09:30:00Z")));
        assert!(!SessionChildStatus::Working.is_unread(wrote, None, None), "not while it is still working");
    }
}
