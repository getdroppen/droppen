//! Claude Code's subagents, as a tree under the session that spawned them
//! ([`droppen_sessions::children`]).
//!
//! Claude Code writes each subagent's transcript beside the session —
//! `<session>/subagents/agent-<id>.jsonl` — with a meta file naming the agent
//! that spawned it (`parentAgentId`) for anything below the first level. A
//! subagent is not a session of its own: it is opened through its parent
//! ([`SessionChildKind::Agent`]).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use droppen_sessions::children::{agent_seen_id, ReadMarksExt, SessionChild, SessionChildKind, SessionChildStatus, SpawnOrderExt};
use super::transcript;
use droppen_sessions::{SessionActivity, SessionSummary, Source};
use droppen_http::openapi_ext::Nullable;

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

impl super::claude::ClaudeCodeSessions {
    /// Claude children of one session, nested by `parentAgentId`.
    pub(super) async fn subagent_tree(&self, session: &SessionSummary, marks: &BTreeMap<String, String>) -> Vec<SessionChild> {
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
                    name: droppen_sessions::util::title_line(&name),
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
        mark_unread(&mut tree, &session.id, marks, marks.read_at(Source::Claude, &session.id));
        tree
    }
}

/// Decide `unread` down a Claude tree, each child against its own mark and
/// the latest one above it.
fn mark_unread(children: &mut [SessionChild], session: &str, marks: &BTreeMap<String, String>, above: Option<&str>) {
    for child in children {
        let own = marks.read_at(Source::Claude, &agent_seen_id(session, &child.id));
        child.unread = child.status.is_unread(&child.updated_at, own, above);
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
        children.sort_by_spawn();
        children
    }
    attach(None, &mut by_parent, 0)
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
    let verdict = match droppen_sessions::util::read_tail_lines(path, TAIL_BYTES).await {
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
        let Some(entry) = droppen_sessions::util::try_parse(line) else { continue };
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
    use droppen_base::time::iso8601;

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
