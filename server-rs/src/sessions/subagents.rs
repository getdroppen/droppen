//! The subagents a session delegated work to, as a tree under it.
//!
//! Both CLIs keep who spawned whom, each in its own way:
//!
//! * **Claude Code** writes each subagent's transcript beside the session —
//!   `<session>/subagents/agent-<id>.jsonl` — with a meta file naming the
//!   agent that spawned it (`parentAgentId`) for anything below the first
//!   level. A subagent is not a session of its own: it is opened through its
//!   parent ([`SessionChildKind::Agent`]).
//! * **Codex** runs a subagent as a thread of its own and records the edge in
//!   its state database (`thread_spawn_edges`). The thread is a session and
//!   opens by its id ([`SessionChildKind::Thread`]); the list keeps it out of
//!   the top level and shows it here instead.
//!
//! Only delegated work is a child. A forked or continued conversation is a
//! session in its own right with no parent link, and stays a top-level row.
//!
//! The tree is an overlay ([`apply`]) like `unread`, computed each time the
//! list is, because a child writes to a file of its own: the parent's
//! transcript can sit still while its children change.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use gotcha::Schematic;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{codex, iso8601, transcript, Paths, SessionActivity, SessionSummary, Source};
use crate::attention::seen;
use crate::openapi_ext::Nullable;

/// How recently a Claude subagent must have written to count as working
/// while its parent is.
const WORKING_WINDOW: Duration = Duration::from_secs(90);
/// How much of a subagent's transcript is read for how it ended.
const TAIL_BYTES: u64 = 32 * 1024;

/// How each subagent transcript ended, while it stays the size and age it was
/// read at. Every listing asks again of every agent of every session, and
/// almost none of them have moved since the last one.
static ENDINGS: LazyLock<Mutex<HashMap<PathBuf, (u64, Option<SystemTime>, bool)>>> = LazyLock::new(Default::default);
/// Past this many transcripts the remembered endings are dropped and read again.
const ENDINGS_LIMIT: usize = 20_000;

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
pub async fn apply(paths: &Paths, sessions: &mut [SessionSummary]) {
    if sessions.is_empty() {
        return;
    }
    // Opt-in (`subagentTree` in the prefs): until it is on, nothing is looked
    // for, and every row goes out with no children, as before the tree.
    if crate::prefs::read_prefs(paths).await.subagent_tree != crate::prefs::SubagentTree::On {
        return;
    }
    let marks = seen::read(paths).await;
    let codex_threads = if sessions.iter().any(|session| session.source == Source::Codex) {
        codex::spawned_threads(paths).await
    } else {
        Vec::new()
    };
    let mut threads_by_parent: HashMap<&str, Vec<&codex::SpawnedThread>> = HashMap::new();
    for thread in &codex_threads {
        threads_by_parent.entry(thread.parent_id.as_str()).or_default().push(thread);
    }
    for session in sessions.iter_mut() {
        session.children = match session.source {
            Source::Claude => claude_children(session, &marks).await,
            Source::Codex => codex_children(&session.id, &threads_by_parent, &marks, read_at(&marks, Source::Codex, &session.id), 0).await,
        };
    }
}

/// The reader's mark on `id`, if they ever opened it.
fn read_at<'a>(marks: &'a BTreeMap<String, String>, source: Source, id: &str) -> Option<&'a str> {
    marks.get(&seen::key_of(source, id)).map(String::as_str)
}

/// The id a Claude subagent's read mark is kept under: within its session.
pub fn agent_seen_id(session_id: &str, agent_id: &str) -> String {
    format!("{session_id}/agents/{agent_id}")
}

/// Whether a child has output its reader has not seen, given the later of its
/// own mark and its parent's.
fn unread(status: SessionChildStatus, updated_at: &str, own: Option<&str>, parent: Option<&str>) -> bool {
    if status == SessionChildStatus::Working {
        return false;
    }
    match own.max(parent) {
        Some(read) => updated_at > read,
        None => true,
    }
}

/// Codex children of `parent`, nested; `depth` guards against a cycle in a
/// damaged edge table.
async fn codex_children(
    parent: &str,
    by_parent: &HashMap<&str, Vec<&codex::SpawnedThread>>,
    marks: &BTreeMap<String, String>,
    parent_read: Option<&str>,
    depth: usize,
) -> Vec<SessionChild> {
    let Some(threads) = by_parent.get(parent).filter(|_| depth < 8) else {
        return Vec::new();
    };
    let mut children = Vec::with_capacity(threads.len());
    for thread in threads {
        let (working, failed) = codex::spawned_thread_state(thread).await;
        let status = match (working, failed) {
            (true, _) => SessionChildStatus::Working,
            (false, true) => SessionChildStatus::Failed,
            (false, false) => SessionChildStatus::Done,
        };
        let updated_at = iso8601(SystemTime::UNIX_EPOCH + Duration::from_millis(thread.updated_ms.max(0) as u64));
        let own = read_at(marks, Source::Codex, &thread.id);
        let read = own.max(parent_read);
        let grandchildren = Box::pin(codex_children(&thread.id, by_parent, marks, read, depth + 1)).await;
        children.push(SessionChild {
            id: thread.id.clone(),
            kind: SessionChildKind::Thread,
            name: thread.name.clone(),
            role: Nullable(thread.role.clone()),
            status,
            unread: unread(status, &updated_at, own, parent_read),
            updated_at,
            children: grandchildren,
        });
    }
    sort(&mut children);
    children
}

/// Claude children of one session, nested by `parentAgentId`.
async fn claude_children(session: &SessionSummary, marks: &BTreeMap<String, String>) -> Vec<SessionChild> {
    let file = Path::new(&session.file_path);
    let agents = transcript::list_session_agents(file).await;
    if agents.is_empty() {
        return Vec::new();
    }
    let parent_working = matches!(session.activity, SessionActivity::Working | SessionActivity::Background);
    let known: std::collections::HashSet<&str> = agents.iter().map(|agent| agent.agent_id.as_str()).collect();
    let mut flat: Vec<(Option<String>, SessionChild)> = Vec::with_capacity(agents.len());
    for agent in &agents {
        let updated_at = agent.updated_at.clone().unwrap_or_default();
        let status = claude_status(agent.transcript_path.as_deref(), &updated_at, parent_working).await;
        let name = agent
            .description
            .clone()
            .filter(|text| !text.trim().is_empty())
            .or_else(|| agent.agent_type.clone())
            .unwrap_or_else(|| format!("agent {}", agent.agent_id.chars().take(8).collect::<String>()));
        // An agent whose parent left no trace hangs off the session itself
        // rather than vanishing with its missing parent.
        let parent = agent.parent_agent_id.clone().filter(|parent| known.contains(parent.as_str()));
        flat.push((
            parent,
            SessionChild {
                id: agent.agent_id.clone(),
                kind: SessionChildKind::Agent,
                name: super::transcript::title_line(&name),
                role: Nullable(agent.agent_type.clone()),
                status,
                // Decided once the tree is built, from the marks above it.
                unread: false,
                updated_at,
                children: Vec::new(),
            },
        ));
    }
    let mut tree = nest(flat);
    mark_unread(&mut tree, &session.id, marks, read_at(marks, Source::Claude, &session.id));
    tree
}

/// Decide `unread` down a Claude tree, each child against its own mark and
/// the latest one above it.
fn mark_unread(children: &mut [SessionChild], session: &str, marks: &BTreeMap<String, String>, above: Option<&str>) {
    for child in children {
        let own = read_at(marks, Source::Claude, &agent_seen_id(session, &child.id));
        child.unread = unread(child.status, &child.updated_at, own, above);
        let below = own.max(above);
        mark_unread(&mut child.children, session, marks, below);
    }
}

/// Build the tree from `(parent, child)` pairs; `None` is the session itself.
fn nest(flat: Vec<(Option<String>, SessionChild)>) -> Vec<SessionChild> {
    let mut by_parent: HashMap<Option<String>, Vec<SessionChild>> = HashMap::new();
    for (parent, child) in flat {
        by_parent.entry(parent).or_default().push(child);
    }
    fn attach(parent: Option<String>, by_parent: &mut HashMap<Option<String>, Vec<SessionChild>>, depth: usize) -> Vec<SessionChild> {
        let mut children = by_parent.remove(&parent).unwrap_or_default();
        if depth < 8 {
            for child in &mut children {
                child.children = attach(Some(child.id.clone()), by_parent, depth + 1);
            }
        }
        sort(&mut children);
        children
    }
    attach(None, &mut by_parent, 0)
}

/// Spawn order, as near as the files tell it: oldest write first, then id.
fn sort(children: &mut [SessionChild]) {
    children.sort_by(|a, b| a.updated_at.cmp(&b.updated_at).then_with(|| a.id.cmp(&b.id)));
}

/// Where a Claude subagent is: working while its parent is and it wrote
/// lately, failed when its transcript ends on an API error or an interruption,
/// done otherwise.
async fn claude_status(path: Option<&Path>, updated_at: &str, parent_working: bool) -> SessionChildStatus {
    let recent = chrono::DateTime::parse_from_rfc3339(updated_at)
        .ok()
        .and_then(|at| SystemTime::now().duration_since(SystemTime::from(at)).ok())
        .is_some_and(|age| age < WORKING_WINDOW);
    if parent_working && recent {
        return SessionChildStatus::Working;
    }
    let Some(path) = path else { return SessionChildStatus::Done };
    if ended_badly(path).await {
        SessionChildStatus::Failed
    } else {
        SessionChildStatus::Done
    }
}

/// Whether a subagent transcript ends on an API error or an interruption,
/// remembered in [`ENDINGS`] until the file changes.
async fn ended_badly(path: &Path) -> bool {
    let Ok(meta) = tokio::fs::metadata(path).await else { return false };
    let stamp = (meta.len(), meta.modified().ok());
    if let Some(&(len, modified, verdict)) = ENDINGS.lock().expect("endings").get(path) {
        if (len, modified) == stamp {
            return verdict;
        }
    }
    let verdict = match super::util::read_tail_lines(path, TAIL_BYTES).await {
        Ok(lines) => tail_ended_badly(&lines),
        Err(_) => return false,
    };
    let mut endings = ENDINGS.lock().expect("endings");
    if endings.len() >= ENDINGS_LIMIT {
        endings.clear();
    }
    endings.insert(path.to_path_buf(), (stamp.0, stamp.1, verdict));
    verdict
}

fn tail_ended_badly(lines: &[String]) -> bool {
    for line in lines.iter().rev() {
        let Some(entry) = super::util::try_parse(line) else { continue };
        match entry.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                let synthetic = entry.get("message").and_then(|message| message.get("model")).and_then(Value::as_str) == Some("<synthetic>");
                let api_error = entry.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true);
                return synthetic || api_error;
            }
            Some("user") => {
                let text = transcript::content_blocks(entry.get("message"))
                    .iter()
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect::<String>();
                return text.trim_start().starts_with("[Request interrupted");
            }
            _ => continue,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child(id: &str, updated_at: &str) -> SessionChild {
        SessionChild {
            id: id.to_string(),
            kind: SessionChildKind::Agent,
            name: id.to_string(),
            role: Nullable(None),
            status: SessionChildStatus::Done,
            updated_at: updated_at.to_string(),
            unread: false,
            children: Vec::new(),
        }
    }

    #[test]
    fn children_nest_under_their_parents_and_orphans_under_the_session() {
        let tree = nest(vec![
            (None, child("a", "2026-10-01T10:00:02Z")),
            (Some("a".into()), child("a1", "2026-10-01T10:00:03Z")),
            (Some("a1".into()), child("a1x", "2026-10-01T10:00:04Z")),
            (None, child("b", "2026-10-01T10:00:01Z")),
        ]);
        let ids: Vec<&str> = tree.iter().map(|child| child.id.as_str()).collect();
        assert_eq!(ids, vec!["b", "a"], "spawn order");
        assert_eq!(tree[1].children[0].id, "a1");
        assert_eq!(tree[1].children[0].children[0].id, "a1x");
    }

    #[test]
    fn a_child_is_unread_until_it_or_its_parent_is_opened_after_it_wrote() {
        let wrote = "2026-10-01T10:00:00Z";
        assert!(unread(SessionChildStatus::Done, wrote, None, None), "never opened");
        assert!(
            !unread(SessionChildStatus::Done, wrote, None, Some("2026-10-01T11:00:00Z")),
            "the parent was read since"
        );
        assert!(
            !unread(SessionChildStatus::Done, wrote, Some("2026-10-01T10:30:00Z"), None),
            "it was read since"
        );
        assert!(unread(
            SessionChildStatus::Failed,
            wrote,
            Some("2026-10-01T09:00:00Z"),
            Some("2026-10-01T09:30:00Z")
        ));
        assert!(!unread(SessionChildStatus::Working, wrote, None, None), "not while it is still working");
    }

    #[tokio::test]
    async fn a_claude_subagent_says_how_it_ended() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("agent.jsonl");
        let old = "2026-01-01T00:00:00Z";
        tokio::fs::write(
            &file,
            r#"{"type":"assistant","message":{"model":"claude-x","content":[{"type":"text","text":"done"}]}}"#,
        )
        .await
        .unwrap();
        assert_eq!(claude_status(Some(&file), old, false).await, SessionChildStatus::Done);
        tokio::fs::write(
            &file,
            r#"{"type":"assistant","isApiErrorMessage":true,"message":{"model":"<synthetic>","content":[]}}"#,
        )
        .await
        .unwrap();
        assert_eq!(claude_status(Some(&file), old, false).await, SessionChildStatus::Failed);
        tokio::fs::write(
            &file,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#,
        )
        .await
        .unwrap();
        assert_eq!(claude_status(Some(&file), old, false).await, SessionChildStatus::Failed);
        // Asked again, it is the remembered answer; written to, it is read again.
        assert_eq!(claude_status(Some(&file), old, false).await, SessionChildStatus::Failed);
        let mut more = tokio::fs::read_to_string(&file).await.unwrap();
        more.push_str("\n{\"type\":\"assistant\",\"message\":{\"model\":\"claude-x\",\"content\":[]}}\n");
        tokio::fs::write(&file, more).await.unwrap();
        assert_eq!(
            claude_status(Some(&file), old, false).await,
            SessionChildStatus::Done,
            "resumed after the interruption"
        );
        let now = iso8601(SystemTime::now());
        assert_eq!(
            claude_status(Some(&file), &now, true).await,
            SessionChildStatus::Working,
            "its parent is working and it just wrote"
        );
    }
}
