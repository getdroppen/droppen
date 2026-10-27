//! The task checklist Claude Code keeps beside a session.
//!
//! The CLI writes one JSON file per task under `<root>/tasks/<session-id>/`,
//! named for the task's number (`1.json`, `2.json`, … `10.json`), and rewrites a
//! file in place as its status changes. Reading the directory is therefore a
//! snapshot of the current plan, with no history to reconcile.
//!
//! Everything here is tolerant: a session with no checklist is the common case,
//! and one unreadable or half-written task file should cost that task, not the
//! whole list. Codex has no equivalent, so this is a Claude-only surface.

use std::path::{Path, PathBuf};

use gotcha::Schematic;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Paths;
use crate::claude_roots::ClaudeRoot;

/// The status the CLI writes for a finished task.
const DONE: &str = "completed";

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

/// Whether an id may name a directory under `tasks/`.
///
/// [`super::claude::find_claude_session`] admits `.` so that an id can carry
/// one, which is harmless where it only ever forms `<id>.jsonl`. Here the id
/// becomes a path *segment*, so `..` would walk out of the tasks directory —
/// hence a stricter rule than the lookup's.
fn is_session_dir_name(id: &str) -> bool {
    // One rule for every id that becomes a path segment of its own.
    super::reference::is_path_segment_id(id)
}

/// `<root>/tasks/<session-id>` — one directory per session, one JSON per task.
pub fn tasks_dir(paths: &Paths, root: ClaudeRoot, session_id: &str) -> Option<PathBuf> {
    is_session_dir_name(session_id).then(|| paths.claude_root(root).join("tasks").join(session_id))
}

/// Parse one task sidecar, or `None` when it is not one.
fn parse_task(stem: &str, raw: &str) -> Option<ChecklistItem> {
    let Ok(Value::Object(task)) = serde_json::from_str::<Value>(raw) else {
        return None;
    };
    let string = |key: &str| task.get(key).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
    // The file name is the task's number, and is the id the CLI would have
    // written anyway — so a sidecar missing the field still lands in order.
    let id = string("id").unwrap_or(stem).to_string();
    let title = string("subject").or_else(|| string("description"))?.to_string();
    let status = string("status").unwrap_or("pending").to_string();
    Some(ChecklistItem { id, title, status })
}

/// Read a session's checklist, which is empty when it has never had one.
pub async fn read_checklist(dir: &Path) -> Checklist {
    let mut numbered: Vec<(u64, String, ChecklistItem)> = Vec::new();
    // No directory is the ordinary case: most sessions never write a task list.
    if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            // `.lock` and `.highwatermark` share the directory with the tasks.
            let Some(stem) = name.strip_suffix(".json").filter(|stem| !stem.is_empty()) else {
                continue;
            };
            let Ok(raw) = tokio::fs::read_to_string(entry.path()).await else {
                continue;
            };
            let Some(item) = parse_task(stem, &raw) else { continue };
            // `10.json` sorts before `2.json` as text; the CLI means it as a number.
            numbered.push((stem.parse::<u64>().unwrap_or(u64::MAX), stem.to_string(), item));
        }
    }
    numbered.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    let items: Vec<ChecklistItem> = numbered.into_iter().map(|(_, _, item)| item).collect();
    let done = items.iter().filter(|item| item.status == DONE).count();
    Checklist {
        total: items.len(),
        done,
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).expect("write");
    }

    fn task(id: &str, subject: &str, status: &str) -> String {
        serde_json::json!({
            "id": id,
            "subject": subject,
            "description": "the long form",
            "activeForm": "doing it",
            "status": status,
            "blocks": [],
            "blockedBy": [],
        })
        .to_string()
    }

    #[tokio::test]
    async fn tasks_come_back_numbered_in_order_with_a_done_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Deliberately out of order, and spanning the 9/10 boundary where a
        // lexicographic sort would put `10` first.
        write(dir.path(), "2.json", &task("2", "second", "pending"));
        write(dir.path(), "10.json", &task("10", "tenth", "pending"));
        write(dir.path(), "1.json", &task("1", "first", "completed"));

        let checklist = read_checklist(dir.path()).await;
        assert_eq!(
            checklist.items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
            ["first", "second", "tenth"]
        );
        assert_eq!((checklist.total, checklist.done), (3, 1));
        assert_eq!(checklist.items[0].status, "completed");
    }

    /// The sidecar directory holds the CLI's own bookkeeping alongside the
    /// tasks, and a task can be caught half-written mid-rewrite.
    #[tokio::test]
    async fn lock_files_and_garbage_cost_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), ".lock", "");
        write(dir.path(), ".highwatermark", "7");
        write(dir.path(), "1.json", &task("1", "real", "in_progress"));
        write(dir.path(), "2.json", "{\"id\":\"2\",\"subj");
        write(dir.path(), "3.json", r#"{"id":"3","status":"pending"}"#);

        let checklist = read_checklist(dir.path()).await;
        assert_eq!(checklist.items.len(), 1, "only the one parseable, titled task survives");
        assert_eq!(checklist.items[0].title, "real");
        assert_eq!(checklist.items[0].status, "in_progress");
        assert_eq!(checklist.done, 0, "an unknown status is not a finished one");
    }

    #[tokio::test]
    async fn a_session_that_never_planned_anything_has_an_empty_checklist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let checklist = read_checklist(&dir.path().join("nope")).await;
        assert_eq!(
            checklist,
            Checklist {
                items: vec![],
                total: 0,
                done: 0
            }
        );
    }

    #[test]
    fn an_id_that_could_walk_out_of_the_tasks_directory_has_no_path() {
        let paths = Paths::new(PathBuf::from("/home/someone"));
        assert!(tasks_dir(&paths, ClaudeRoot::Local, "..").is_none());
        assert!(tasks_dir(&paths, ClaudeRoot::Local, "a/b").is_none());
        assert!(tasks_dir(&paths, ClaudeRoot::Local, "").is_none());
        assert!(tasks_dir(&paths, ClaudeRoot::Local, "0fca2654-3dfe-48ba-a87d-defad50d8a52").is_some());
    }
}
