//! Codex discovery, end to end, over both of its independent paths.
//!
//! The two are exercised against the *same* synthetic `~/.codex` tree: with the
//! state database present the thread index decides what is visible, and with it
//! absent the rollout files are scanned instead. Running both over one tree is
//! the point — it is what proves the fallback is a real substitute rather than a
//! differently shaped answer.

mod common;

use std::path::Path;

use futures_util::StreamExt;
use serde_json::Value;

const SESSION_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const ARCHIVED_ID: &str = "11111111-2222-3333-4444-555555555555";
const SUBAGENT_ID: &str = "99999999-8888-7777-6666-555555555555";

/// Record a thread as one this server created.
///
/// Codex ownership cannot be read off a path — there is one `~/.codex` — so it
/// is a registry in the instance database. The server itself writes only the
/// table, so the fixture inserts the row it would have written.
fn own_codex_session(home: &Path, id: &str) {
    let paths = remote_hand_server::sessions::Paths::new(home.to_path_buf());
    remote_hand_server::db::at(paths.instance_db())
        .write(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO owned_sessions (source, session_id) VALUES ('codex', ?1)",
                (id,),
            )
        })
        .expect("seed the ownership registry");
}

/// One rollout file, in the `sessions/YYYY/MM/DD` tree Codex writes.
fn seed_rollout(home: &Path, id: &str, lines: &[String]) -> std::path::PathBuf {
    let dir = home.join(".codex/sessions/2026/05/09");
    std::fs::create_dir_all(&dir).expect("rollout dir");
    let path = dir.join(format!("rollout-2026-05-09T16-39-49-{id}.jsonl"));
    std::fs::write(&path, format!("{}\n", lines.join("\n"))).expect("seed rollout");
    path
}

/// A session with a working directory, a question, some reasoning and a tool call.
fn transcript() -> Vec<String> {
    [
        r#"{"type":"session_meta","timestamp":"2026-05-09T16:39:49.000Z","payload":{"id":"x","cwd":"/Users/me/proj"}}"#,
        r#"{"type":"turn_context","timestamp":"2026-05-09T16:39:50.000Z","payload":{"model":"gpt-5.5","effort":"high"}}"#,
        r#"{"type":"response_item","timestamp":"2026-05-09T16:39:51.000Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>ignored</environment_context>"}]}}"#,
        r#"{"type":"response_item","timestamp":"2026-05-09T16:39:52.000Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"port the codex half"}]}}"#,
        r#"{"type":"response_item","timestamp":"2026-05-09T16:39:53.000Z","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"cargo test\",\"workdir\":\"/Users/me/proj\"}"}}"#,
        r#"{"type":"response_item","timestamp":"2026-05-09T16:39:54.000Z","payload":{"type":"function_call_output","output":"{\"exit_code\":0,\"output\":\"ok\"}"}}"#,
        r#"{"type":"response_item","timestamp":"2026-05-09T16:39:55.000Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}"#,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// A `threads` table shaped like Codex's own, holding one visible thread plus
/// the two kinds that must stay hidden.
fn seed_state_db(home: &Path, rollout: &Path) {
    let db = home.join(".codex/state_5.sqlite");
    let connection = rusqlite::Connection::open(&db).expect("create state db");
    connection
        .execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY, title TEXT, first_user_message TEXT, cwd TEXT,
                git_branch TEXT, updated_at INTEGER, updated_at_ms INTEGER,
                rollout_path TEXT, archived INTEGER, source TEXT, created_at INTEGER
             );",
        )
        .expect("create threads");
    let path = rollout.to_string_lossy().into_owned();
    let mut insert = connection
        .prepare("INSERT INTO threads VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .expect("prepare insert");
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as i64;
    insert
        .execute(rusqlite::params![
            SESSION_ID,
            "indexed title",
            "port the codex half",
            "/Users/me/proj",
            "main",
            now_ms / 1000,
            now_ms,
            path,
            0,
            "cli",
            now_ms / 1000
        ])
        .expect("insert visible");
    insert
        .execute(rusqlite::params![
            ARCHIVED_ID,
            "archived thread",
            "",
            "/Users/me/proj",
            Option::<String>::None,
            now_ms / 1000,
            now_ms,
            path,
            1,
            "cli",
            now_ms / 1000
        ])
        .expect("insert archived");
    insert
        .execute(rusqlite::params![
            SUBAGENT_ID,
            "subagent thread",
            "",
            "/Users/me/proj",
            Option::<String>::None,
            now_ms / 1000,
            now_ms,
            path,
            0,
            "{\"parent\":\"x\"}",
            now_ms / 1000
        ])
        .expect("insert subagent");
}

#[tokio::test]
async fn codex_file_citations_are_downloadable_in_details_and_live_snapshots() {
    let server = common::TestServer::start_with(|home| {
        let output = home.join("reports/每周分析/综合策略（核查版）.xlsx");
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(&output, b"PK\0test output").unwrap();
        std::fs::write(home.join("HANDOFF.md"), "# Handoff\nready").unwrap();
        let mut lines = transcript();
        lines[0] = serde_json::json!({"type":"session_meta","payload":{"id":SESSION_ID,"cwd":home}}).to_string();
        lines.push(serde_json::json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{
            "type":"output_text","text":format!("生成综合核查版：:codex-file-citation{{path={} purpose=\"output\"}}，另见 `HANDOFF.md`。", serde_json::to_string(&output).unwrap())
        }]}}).to_string());
        seed_rollout(home, SESSION_ID, &lines);
    }).await;

    let detail = server.get_json(&format!("/api/sessions/codex/{SESSION_ID}")).await;
    let last = detail["items"].as_array().unwrap().last().unwrap();
    assert_eq!(last["files"].as_array().unwrap().len(), 2);
    assert_eq!(last["files"][0]["name"], "综合策略（核查版）.xlsx");
    assert_eq!(
        last["files"][1]["name"], "HANDOFF.md",
        "ordinary Codex file mentions use the same annotation now"
    );
    assert!(!last["text"].as_str().unwrap().contains(":codex-file-citation"));

    let (mut ws, _) = tokio_tungstenite::connect_async(server.ws_url(&format!("/api/sessions/codex/{SESSION_ID}/ws")))
        .await
        .unwrap();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let snapshot: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
    assert_eq!(snapshot["type"], "snapshot");
    assert_eq!(snapshot["items"].as_array().unwrap().last().unwrap()["files"], last["files"]);

    let path = last["files"][0]["path"].as_str().unwrap();
    let encoded: String = path.as_bytes().iter().map(|b| format!("%{b:02X}")).collect();
    let preview = server.get_json(&format!("/api/fs/read?path={encoded}")).await;
    assert_eq!(preview["binary"], true);
    let (status, headers, body) = server.request_full("GET", &format!("/api/fs/download?path={encoded}"), &[], &[]).await;
    assert_eq!(status, 200);
    assert!(headers.to_lowercase().contains("content-disposition: attachment"));
    assert_eq!(body, "PK\0test output");
    ws.close(None).await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn scanning_finds_a_session_when_there_is_no_state_database() {
    let server = common::TestServer::start_with(|home| {
        seed_rollout(home, SESSION_ID, &transcript());
    })
    .await;

    // A rollout this server did not make is external, so it takes `scope=all`
    // (or `scope=external`) to see it.
    assert_eq!(server.get_json("/api/sessions?source=codex").await["total"], 0);
    let body = server.get_json("/api/sessions?source=codex&scope=all").await;
    assert_eq!(body["total"], 1);
    let session = &body["sessions"][0];
    assert_eq!(session["scope"], "external", "nothing recorded it as ours");
    assert_eq!(session["id"], SESSION_ID, "the id comes out of the rollout file name");
    assert_eq!(session["source"], "codex");
    assert_eq!(session["title"], "port the codex half", "a wrapper preamble is never the title");
    assert_eq!(session["cwd"], "/Users/me/proj", "session_meta carries the working directory");
    assert_eq!(session["gitBranch"], Value::Null, "a scanned session has no branch to report");
    assert_eq!(session["claudeRoot"], Value::Null, "a codex session belongs to no claude root");
    assert_eq!(session["status"], "waiting", "the last entry is an assistant message");

    server.stop().await;
}

#[tokio::test]
async fn the_state_database_wins_and_hides_archived_and_subagent_threads() {
    let server = common::TestServer::start_with(|home| {
        let rollout = seed_rollout(home, SESSION_ID, &transcript());
        seed_state_db(home, &rollout);
    })
    .await;

    let body = server.get_json("/api/sessions?source=codex&scope=all").await;
    assert_eq!(body["total"], 1, "archived and subagent threads never reach the list");
    let session = &body["sessions"][0];
    assert_eq!(session["id"], SESSION_ID);
    assert_eq!(session["title"], "indexed title", "the database title beats the derived one");
    assert_eq!(session["gitBranch"], "main", "only the database knows the branch");

    let (status, _) = server.get(&format!("/api/sessions/codex/{ARCHIVED_ID}")).await;
    assert_eq!(status, 404, "an archived thread is not addressable either");

    server.stop().await;
}

#[tokio::test]
async fn a_codex_detail_carries_its_transcript_and_its_model() {
    let server = common::TestServer::start_with(|home| {
        seed_rollout(home, SESSION_ID, &transcript());
        // Owned, so the composer's verdict is about the session's state rather
        // than about whose session it is.
        own_codex_session(home, SESSION_ID);
    })
    .await;

    let detail = server.get_json(&format!("/api/sessions/codex/{SESSION_ID}")).await;
    assert_eq!(detail["summary"]["id"], SESSION_ID);
    assert_eq!(detail["totalItems"], 4, "the wrapper message and session_meta are not items");
    assert_eq!(detail["items"][0]["role"], "user");
    assert_eq!(detail["items"][0]["text"], "port the codex half");
    assert_eq!(detail["items"][1]["kind"], "tool_use");
    assert_eq!(detail["items"][1]["text"], "cargo test");
    assert_eq!(detail["items"][2]["kind"], "tool_result");
    assert_eq!(detail["items"][2]["meta"], "exit 0", "the exec envelope is unwrapped");
    assert_eq!(detail["items"][3]["text"], "done");
    assert_eq!(detail["cliChoice"]["model"], "gpt-5.5", "turn_context is the model of record");
    assert_eq!(detail["cliChoice"]["effort"], "high");
    assert_eq!(detail["summary"]["scope"], "owned", "this server's own record claims it");
    assert_eq!(detail["canSend"], true, "a waiting codex session can be sent to");

    let (status, body) = server.get("/api/sessions/codex/does-not-exist").await;
    assert_eq!(status, 404);
    assert_eq!(serde_json::from_str::<Value>(&body).expect("json")["error"], "session not found");

    server.stop().await;
}

#[tokio::test]
async fn the_session_feed_streams_a_codex_snapshot() {
    let server = common::TestServer::start_with(|home| {
        seed_rollout(home, SESSION_ID, &transcript());
    })
    .await;

    let url = server.ws_url(&format!("/api/sessions/codex/{SESSION_ID}/ws"));
    let (socket, _) = tokio_tungstenite::connect_async(&url).await.expect("websocket connect");
    let (_sink, mut stream) = socket.split();

    let frame = tokio::time::timeout(std::time::Duration::from_secs(10), stream.next())
        .await
        .expect("timed out waiting for the snapshot")
        .expect("stream ended")
        .expect("websocket error");
    let snapshot: Value = serde_json::from_str(&frame.into_text().expect("text frame")).expect("json frame");

    assert_eq!(snapshot["type"], "snapshot");
    assert_eq!(snapshot["totalItems"], 4);
    assert_eq!(snapshot["summary"]["source"], "codex");
    assert_eq!(snapshot["items"][3]["text"], "done");

    server.stop().await;
}
