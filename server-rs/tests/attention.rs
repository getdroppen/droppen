//! Unread, end to end: derived off a real transcript, cleared by a real request.
//!
//! The unit tests in `src/attention` pin the two halves separately — which
//! transcript entries move the clock, and how the clock compares to `read_at`.
//! What is only observable from out here is that the *same verdict* reaches
//! every surface: the list route, the detail route and the websocket rows all
//! serve one row, and a badge that disagreed with itself between the sidebar and
//! the session header would be worse than no badge.

mod common;

use std::path::{Path, PathBuf};

use common::TestServer;
use remote_hand_server::db::scope;
use serde_json::{json, Value};

const IDLE_ID: &str = "11111111-1111-4111-8111-111111111111";
const LIVE_ID: &str = "22222222-2222-4222-8222-222222222222";
const SIDECHAIN_ID: &str = "33333333-3333-4333-8333-333333333333";
const SILENT_ID: &str = "44444444-4444-4444-8444-444444444444";

/// Write a transcript of raw JSONL entries into the local Claude root.
fn seed(home: &Path, id: &str, entries: &[Value]) -> PathBuf {
    common::seed_transcript(&home.join(".claude"), "-workspace", id, entries)
}

fn user(at: &str, text: &str) -> Value {
    json!({"type": "user", "cwd": "/workspace", "timestamp": at, "message": {"content": [{"type": "text", "text": text}]}})
}

fn assistant(at: &str, text: &str) -> Value {
    json!({"type": "assistant", "timestamp": at, "message": {"content": [{"type": "text", "text": text}]}})
}

/// Register `id` as having a live CLI attached, using this process's own pid —
/// which is unarguably alive, and is how discovery decides a session is running.
fn seed_live(home: &Path, id: &str) {
    let dir = home.join(".claude/sessions");
    std::fs::create_dir_all(&dir).expect("sessions dir");
    std::fs::write(
        dir.join(format!("{id}.json")),
        json!({"pid": std::process::id(), "sessionId": id}).to_string(),
    )
    .expect("live registration");
}

/// The list row for one session.
///
/// `scope=all`: these fixtures live in the synthetic `~/.claude`, which is
/// external to the server looking at it, and attention is a property of every
/// row rather than only of the ones this server made.
async fn row(server: &TestServer, id: &str) -> Value {
    let list = server.get_json("/api/sessions?scope=all").await;
    list["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|row| row["id"] == id)
        .unwrap_or_else(|| panic!("no row for {id} in {list}"))
        .clone()
}

#[tokio::test]
async fn a_session_is_unread_from_the_assistants_last_word_until_it_is_seen() {
    let server = TestServer::start_with(|home| {
        seed(
            home,
            IDLE_ID,
            &[
                user("2026-01-01T10:00:00.000Z", "do the thing"),
                assistant("2026-01-01T10:00:09.000Z", "done, have a look"),
            ],
        );
    })
    .await;

    let before = row(&server, IDLE_ID).await;
    assert_eq!(
        before["attentionAt"], "2026-01-01T10:00:09.000Z",
        "the clock is the assistant's own timestamp, not the file's mtime"
    );
    assert_eq!(before["unread"], true, "never looked at, and the assistant has spoken");

    // The detail route serves the same row, so it must agree.
    let detail = server.get_json(&format!("/api/sessions/claude/{IDLE_ID}")).await;
    assert_eq!(detail["summary"]["unread"], true);
    assert_eq!(detail["summary"]["attentionAt"], before["attentionAt"]);

    let (status, body) = server.json("POST", &format!("/api/sessions/claude/{IDLE_ID}/seen"), &json!({})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(serde_json::from_str::<Value>(&body).expect("json")["accepted"], true);

    let after = row(&server, IDLE_ID).await;
    assert_eq!(after["unread"], false, "marked seen after the assistant last spoke");
    assert_eq!(after["attentionAt"], before["attentionAt"], "seeing it does not move the clock");

    let detail = server.get_json(&format!("/api/sessions/claude/{IDLE_ID}")).await;
    assert_eq!(detail["summary"]["unread"], false, "and the detail route says the same");

    server.stop().await;
}

/// The badge answers "is this waiting on me", and a session mid-turn is not.
/// Its last sentence is genuinely unread; it is genuinely not your problem yet.
#[tokio::test]
async fn a_running_session_is_never_unread_however_old_its_last_word_is() {
    let server = TestServer::start_with(|home| {
        seed(
            home,
            LIVE_ID,
            &[
                assistant("2026-01-01T10:00:00.000Z", "here is what I found"),
                // A fresh user turn with a live process is what discovery reads
                // as "running".
                user("2026-01-01T10:05:00.000Z", "now do the next bit"),
            ],
        );
        seed_live(home, LIVE_ID);
    })
    .await;

    let row = row(&server, LIVE_ID).await;
    assert_eq!(row["status"], "running", "the fixture is actually mid-turn");
    assert_eq!(
        row["attentionAt"], "2026-01-01T10:00:00.000Z",
        "the clock still points at the last thing that was said"
    );
    assert_eq!(row["unread"], false, "…but a session that is working is not waiting on anyone");

    server.stop().await;
}

/// A Task subagent talks for as long as it runs and the user sees none of it.
/// If those lines moved the clock, every long delegation would show up unread
/// the entire time it was delegating.
#[tokio::test]
async fn a_subagents_chatter_does_not_make_a_session_unread() {
    let server = TestServer::start_with(|home| {
        let entries = vec![
            assistant("2026-01-01T10:00:00.000Z", "delegating this"),
            json!({
                "type": "assistant",
                "isSidechain": true,
                "timestamp": "2026-01-01T11:00:00.000Z",
                "message": {"content": [{"type": "text", "text": "subagent progress"}]},
            }),
            json!({
                "type": "assistant",
                "timestamp": "2026-01-01T11:30:00.000Z",
                "message": {"content": [{"type": "tool_use", "name": "Bash", "input": {}}]},
            }),
        ];
        seed(home, SIDECHAIN_ID, &entries);
    })
    .await;

    let row = row(&server, SIDECHAIN_ID).await;
    assert_eq!(
        row["attentionAt"], "2026-01-01T10:00:00.000Z",
        "neither the sidechain nor the tool call is the assistant speaking to the user"
    );

    server.stop().await;
}

#[tokio::test]
async fn a_session_the_assistant_has_not_spoken_in_has_no_clock_and_is_not_unread() {
    let server = TestServer::start_with(|home| {
        seed(home, SILENT_ID, &[user("2026-01-01T10:00:00.000Z", "just asked")]);
    })
    .await;

    let row = row(&server, SILENT_ID).await;
    assert_eq!(row["attentionAt"], Value::Null, "nothing has been said yet");
    assert_eq!(row["unread"], false, "so there is nothing to have missed");

    server.stop().await;
}

/// The store is where `read_at` lives; a restart must not un-read everything.
///
/// The key is the interesting half and the reason this is asked of the store
/// rather than of the `unread` flag the list already reports: `"<source>/<id>"`
/// is *shared*, so the phone marking a session read clears the badge on the
/// laptop and on the other instance running on this machine. A mark written
/// under some per-server key would look identical from this server and be
/// invisible from every other one.
#[tokio::test]
async fn a_mark_is_stored_under_the_key_every_server_reads() {
    let server = TestServer::start_with(|home| {
        seed(home, IDLE_ID, &[assistant("2026-01-01T10:00:09.000Z", "done")]);
    })
    .await;

    server.json("POST", &format!("/api/sessions/claude/{IDLE_ID}/seen"), &json!({})).await;

    let stored = common::rows(
        &server.paths.config_db(),
        "SELECT value FROM kv WHERE scope = ?1 AND key = ?2",
        (scope::SEEN, format!("claude/{IDLE_ID}")),
    );
    let at = stored.first().expect("a timestamp under source/id").as_str();
    assert!(at.ends_with('Z'), "ISO-8601 UTC, like every other clock in the contract: {at}");
    assert!(
        at.as_bytes().first().is_some_and(|b| *b == b'2'),
        "and an actual date rather than a placeholder: {at}"
    );

    server.stop().await;
}
