//! The subagent tree under a session, end to end, for both CLIs.
//!
//! Claude Code keeps a subagent as a transcript beside its session, named in a
//! meta file; Codex keeps one as a thread of its own, with the edge in its state
//! database. Both are seeded here as the CLIs write them, and read back through
//! the list, the detail and the read-mark routes.

mod common;

use std::path::Path;

use common::TestServer;
use serde_json::{json, Value};

const CLAUDE_SESSION: &str = "11111111-1111-4111-8111-111111111111";
const CODEX_PARENT: &str = "aaaaaaaa-0000-4000-8000-000000000001";
const CODEX_CHILD: &str = "aaaaaaaa-0000-4000-8000-000000000002";
const CODEX_GRANDCHILD: &str = "aaaaaaaa-0000-4000-8000-000000000003";
const CODEX_GUARDIAN: &str = "aaaaaaaa-0000-4000-8000-000000000004";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("dir");
    std::fs::write(path, text).expect("write");
}

/// A Claude session with a subagent that delegated to one of its own.
fn seed_claude(home: &Path) {
    let project = home.join(".claude/projects/-work");
    let line = |text: &str| {
        json!({"type": "user", "cwd": "/work", "timestamp": "2026-10-01T10:00:00.000Z",
               "message": {"content": [{"type": "text", "text": text}]}})
        .to_string()
    };
    write(
        &project.join(format!("{CLAUDE_SESSION}.jsonl")),
        &format!("{}\n", line("review the change")),
    );
    let agents = project.join(CLAUDE_SESSION).join("subagents");
    let answer = json!({"type": "assistant", "isSidechain": true, "timestamp": "2026-10-01T10:01:00.000Z",
                        "message": {"model": "claude-x", "content": [{"type": "text", "text": "looks fine"}]}})
    .to_string();
    write(&agents.join("agent-a1.jsonl"), &format!("{answer}\n"));
    write(
        &agents.join("agent-a1.meta.json"),
        &json!({"agentType": "general-purpose", "description": "Review the diff", "toolUseId": "toolu_1", "spawnDepth": 1}).to_string(),
    );
    write(&agents.join("agent-a2.jsonl"), &format!("{answer}\n"));
    write(
        &agents.join("agent-a2.meta.json"),
        &json!({"agentType": "Explore", "description": "Find the callers", "spawnDepth": 2, "parentAgentId": "a1"}).to_string(),
    );
}

/// A Codex thread with a delegated child, a grandchild, and a guardian review
/// nobody delegated.
fn seed_codex(home: &Path) {
    let rollout = |id: &str, last: Value| {
        let path = home.join(format!(".codex/sessions/2026/10/01/rollout-2026-10-01T10-00-00-{id}.jsonl"));
        let lines = [
            json!({"type": "session_meta", "timestamp": "2026-10-01T10:00:00.000Z", "payload": {"id": id, "cwd": "/work"}}),
            json!({"type": "response_item", "timestamp": "2026-10-01T10:00:01.000Z",
                   "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "go"}]}}),
            last,
        ];
        write(&path, &(lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n"));
        path.to_string_lossy().into_owned()
    };
    let complete = json!({"type": "event_msg", "timestamp": "2026-10-01T10:00:02.000Z", "payload": {"type": "task_complete"}});
    let failed = json!({"type": "event_msg", "timestamp": "2026-10-01T10:00:02.000Z", "payload": {"type": "error", "message": "boom"}});
    std::fs::create_dir_all(home.join(".codex")).expect("codex dir");
    let db = rusqlite::Connection::open(home.join(".codex/state_5.sqlite")).expect("state db");
    db.execute_batch(
        "CREATE TABLE threads (
            id TEXT PRIMARY KEY, title TEXT, first_user_message TEXT, cwd TEXT, git_branch TEXT,
            updated_at INTEGER, updated_at_ms INTEGER, rollout_path TEXT, archived INTEGER, source TEXT,
            created_at INTEGER, agent_nickname TEXT, agent_role TEXT
         );
         CREATE TABLE thread_spawn_edges (parent_thread_id TEXT NOT NULL, child_thread_id TEXT NOT NULL PRIMARY KEY, status TEXT NOT NULL);",
    )
    .expect("schema");
    let then = 1_790_000_000_i64;
    // The failed one wrote just now: an error that recent must still read as
    // failed, not as a turn in flight.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64;
    let mut insert = db
        .prepare("INSERT INTO threads VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?)")
        .expect("insert");
    let spawned = |parent: &str| json!({"subagent": {"thread_spawn": {"parent_thread_id": parent, "depth": 1}}}).to_string();
    for (id, title, source, nickname, role, last) in [
        (CODEX_PARENT, "plan the migration", "cli".to_string(), None, None, complete.clone()),
        (
            CODEX_CHILD,
            "survey the schema",
            spawned(CODEX_PARENT),
            Some("Turing"),
            Some("explorer"),
            complete.clone(),
        ),
        (
            CODEX_GRANDCHILD,
            "count the rows",
            spawned(CODEX_CHILD),
            Some("Ampere"),
            None,
            failed.clone(),
        ),
        (
            CODEX_GUARDIAN,
            "Guardian review",
            json!({"subagent": {"other": "guardian"}}).to_string(),
            None,
            None,
            complete.clone(),
        ),
    ] {
        let path = rollout(id, last);
        let at = if id == CODEX_GRANDCHILD { now } else { then };
        insert
            .execute(rusqlite::params![
                id,
                title,
                title,
                "/work",
                "main",
                at,
                at * 1000,
                path,
                source,
                at,
                nickname,
                role
            ])
            .expect("thread");
    }
    for (parent, child) in [(CODEX_PARENT, CODEX_CHILD), (CODEX_CHILD, CODEX_GRANDCHILD)] {
        db.execute("INSERT INTO thread_spawn_edges VALUES (?, ?, 'open')", [parent, child])
            .expect("edge");
    }
}

/// The tree is opt-in: turn it on, as the settings page does.
async fn enable_tree(server: &TestServer) {
    let prefs = json!({"sessionGrouping": "flat", "composerSend": "enter", "fontSize": "default", "language": "auto", "transcriptStyle": "flat", "subagentTree": "on"});
    let (status, body) = server.json("PUT", "/api/prefs", &prefs).await;
    assert_eq!(status, 200, "{body}");
}

fn row<'a>(list: &'a Value, id: &str) -> Option<&'a Value> {
    list["sessions"].as_array().expect("sessions").iter().find(|row| row["id"] == id)
}

#[tokio::test]
async fn claude_subagents_nest_under_their_session_and_are_read_one_by_one() {
    let server = TestServer::start_with(seed_claude).await;
    let list = server.get_json("/api/sessions?scope=all").await;
    assert_eq!(
        row(&list, CLAUDE_SESSION).expect("the session lists")["children"],
        json!([]),
        "off until turned on"
    );
    let detail = server.get_json(&format!("/api/sessions/claude/{CLAUDE_SESSION}")).await;
    assert_eq!(detail["summary"]["children"], json!([]), "on the detail too");
    enable_tree(&server).await;
    let list = server.get_json("/api/sessions?scope=all").await;
    let session = row(&list, CLAUDE_SESSION).expect("the session lists");
    let children = session["children"].as_array().expect("children");
    assert_eq!(children.len(), 1, "the nested agent hangs off its parent agent: {session:#}");
    let first = &children[0];
    assert_eq!(
        (
            first["id"].as_str(),
            first["kind"].as_str(),
            first["name"].as_str(),
            first["role"].as_str()
        ),
        (Some("a1"), Some("agent"), Some("Review the diff"), Some("general-purpose"))
    );
    assert_eq!(first["status"], "done");
    assert_eq!(first["unread"], true, "never opened");
    assert_eq!(first["children"][0]["id"], "a2");
    assert_eq!(first["children"][0]["name"], "Find the callers");

    // Opening one marks it read, and what it delegated: its conversation
    // shows that work. Not its siblings' or its parent's.
    let (status, body) = server
        .json("POST", &format!("/api/sessions/claude/{CLAUDE_SESSION}/agents/a1/seen"), &json!({}))
        .await;
    assert_eq!(status, 200, "{body}");
    let list = server.get_json("/api/sessions?scope=all").await;
    let first = &row(&list, CLAUDE_SESSION).unwrap()["children"][0];
    assert_eq!(first["unread"], false);
    assert_eq!(first["children"][0]["unread"], false);

    // Opening only the nested one leaves the one above it unread.
    let server2 = TestServer::start_with(seed_claude).await;
    enable_tree(&server2).await;
    server2
        .json("POST", &format!("/api/sessions/claude/{CLAUDE_SESSION}/agents/a2/seen"), &json!({}))
        .await;
    let list = server2.get_json("/api/sessions?scope=all").await;
    let first = &row(&list, CLAUDE_SESSION).unwrap()["children"][0];
    assert_eq!(
        (first["unread"].as_bool(), first["children"][0]["unread"].as_bool()),
        (Some(true), Some(false))
    );
    // And opening the session itself covers everything it delegated.
    server2
        .json("POST", &format!("/api/sessions/claude/{CLAUDE_SESSION}/seen"), &json!({}))
        .await;
    let list = server2.get_json("/api/sessions?scope=all").await;
    assert_eq!(row(&list, CLAUDE_SESSION).unwrap()["children"][0]["unread"], false);
    server2.stop().await;

    let (status, _) = server
        .json("POST", &format!("/api/sessions/claude/{CLAUDE_SESSION}/agents/..%2Fx/seen"), &json!({}))
        .await;
    assert_eq!(status, 400, "an agent id that could name a path is refused");
    server.stop().await;
}

#[tokio::test]
async fn codex_subagent_threads_nest_under_their_parent_and_open_with_a_way_back() {
    let server = TestServer::start_with(seed_codex).await;
    let list = server.get_json("/api/sessions?scope=all").await;
    assert!(
        row(&list, CODEX_CHILD).is_none(),
        "a subagent thread is not a top-level row, tree or no tree"
    );
    assert_eq!(
        row(&list, CODEX_PARENT).expect("the parent lists")["children"],
        json!([]),
        "off until turned on"
    );
    enable_tree(&server).await;
    let list = server.get_json("/api/sessions?scope=all").await;
    assert!(row(&list, CODEX_GUARDIAN).is_none(), "nor is a guardian review");
    let parent = row(&list, CODEX_PARENT).expect("the parent lists");
    let children = parent["children"].as_array().expect("children");
    assert_eq!(children.len(), 1, "only delegated work: {parent:#}");
    let child = &children[0];
    assert_eq!(
        (
            child["id"].as_str(),
            child["kind"].as_str(),
            child["name"].as_str(),
            child["role"].as_str()
        ),
        (Some(CODEX_CHILD), Some("thread"), Some("Turing"), Some("explorer"))
    );
    assert_eq!(child["status"], "done");
    let grandchild = &child["children"][0];
    assert_eq!(
        (grandchild["id"].as_str(), grandchild["status"].as_str()),
        (Some(CODEX_GRANDCHILD), Some("failed"))
    );

    // A child thread opens by its id, and says whose it is.
    let detail = server.get_json(&format!("/api/sessions/codex/{CODEX_CHILD}")).await;
    assert_eq!(detail["summary"]["parentId"], CODEX_PARENT);
    assert_eq!(detail["summary"]["title"], "Turing", "titled as its row under the parent is");
    assert_eq!(detail["summary"]["children"][0]["id"], CODEX_GRANDCHILD, "and what it delegated");
    let (status, _) = server.get(&format!("/api/sessions/codex/{CODEX_GUARDIAN}")).await;
    assert_eq!(status, 404, "a guardian review stays hidden");

    // Read through its own session route.
    assert_eq!(child["unread"], true);
    server.json("POST", &format!("/api/sessions/codex/{CODEX_CHILD}/seen"), &json!({})).await;
    let list = server.get_json("/api/sessions?scope=all").await;
    let child = &row(&list, CODEX_PARENT).unwrap()["children"][0];
    assert_eq!(child["unread"], false);
    assert_eq!(child["children"][0]["unread"], false, "nor is what it delegated");
    server.stop().await;
}
