//! The global feed, end to end: connect, receive the opening frames, change a
//! transcript, receive the delta.
//!
//! The delta is what the sidebar lives on, so the test drives it the way the
//! product does — by writing to a transcript file rather than by calling an
//! internal refresh. Whether the filesystem watcher or the 5 s poll notices is
//! deliberately not asserted: both are legitimate, and under this repository's
//! agent sandbox FSEvents does not reach a `cargo test` binary at all.

mod common;

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;

const SESSION_ID: &str = "11111111-2222-3333-4444-555555555555";

/// The read half of a connected feed socket.
type FeedStream = futures_util::stream::SplitStream<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>>;

/// Fifteen seconds is generous on purpose: where the platform's watcher is
/// unavailable the 5 s poll is what delivers, and a loaded machine can miss one.
async fn next_frame(stream: &mut FeedStream) -> Value {
    let message = tokio::time::timeout(Duration::from_secs(15), stream.next())
        .await
        .expect("timed out waiting for a frame")
        .expect("stream ended")
        .expect("websocket error");
    serde_json::from_str(&message.into_text().expect("text frame")).expect("json frame")
}

/// The next frame of a given type, skipping any that arrive first.
async fn next_frame_of(stream: &mut FeedStream, kind: &str) -> Value {
    loop {
        let frame = next_frame(stream).await;
        if frame["type"] == kind {
            return frame;
        }
    }
}

fn seed_session(home: &Path) {
    common::seed_transcript(
        &home.join(".claude"),
        "-Users-me-proj",
        SESSION_ID,
        &[serde_json::json!({"type":"user","cwd":"/Users/me/proj","timestamp":"2026-05-01T10:00:00.000Z","message":{"content":[{"type":"text","text":"watch me"}]}})],
    );
}

#[tokio::test]
async fn pin_changes_reach_two_clients_and_survive_server_restart() {
    let home = tempfile::tempdir().unwrap();
    seed_session(home.path());
    let paths = remote_hand_server::sessions::Paths::new(home.path().to_owned());
    let managed = paths.claude_managed_projects().join("project");
    std::fs::create_dir_all(&managed).unwrap();
    std::fs::rename(
        home.path().join(format!(".claude/projects/-Users-me-proj/{SESSION_ID}.jsonl")),
        managed.join(format!("{SESSION_ID}.jsonl")),
    )
    .unwrap();
    let server = common::TestServer::start_at(home.path(), None).await;
    let (a, _) = tokio_tungstenite::connect_async(server.ws_url("/api/events")).await.unwrap();
    let (b, _) = tokio_tungstenite::connect_async(server.ws_url("/api/events")).await.unwrap();
    let (_, mut a) = a.split();
    let (_, mut b) = b.split();
    assert_eq!(next_frame_of(&mut a, "sessions:snapshot").await["sessions"][0]["pinned"], false);
    assert_eq!(next_frame_of(&mut b, "sessions:snapshot").await["sessions"][0]["pinned"], false);
    let route = format!("/api/sessions/claude/{SESSION_ID}/pin");
    for pinned in [true, false, true] {
        let body = serde_json::json!({"pinned": pinned}).to_string();
        assert_eq!(
            server
                .request("PUT", &route, &[("Content-Type", "application/json")], body.as_bytes())
                .await
                .0,
            200
        );
        for stream in [&mut a, &mut b] {
            let frame = next_frame_of(stream, "sessions:delta").await;
            assert_eq!(frame["upserts"][0]["id"], SESSION_ID);
            assert_eq!(frame["upserts"][0]["pinned"], pinned);
        }
    }
    drop(a);
    drop(b);
    server.stop().await;
    let restarted = common::TestServer::start_at(home.path(), None).await;
    assert_eq!(restarted.get_json("/api/sessions").await["sessions"][0]["pinned"], true);
    restarted.stop().await;
}

#[tokio::test]
async fn the_feed_opens_with_settings_and_a_snapshot_then_pushes_a_delta() {
    let server = common::TestServer::start_with(seed_session).await;
    let (socket, _) = tokio_tungstenite::connect_async(&server.ws_url("/api/events"))
        .await
        .expect("websocket connect");
    let (_sink, mut stream) = socket.split();

    let settings = next_frame(&mut stream).await;
    assert_eq!(settings["type"], "settings", "the server describes itself first");
    assert_eq!(settings["homeDir"], server.home.to_string_lossy().into_owned());
    assert_eq!(settings["claudeDir"], server.paths.claude_projects().to_string_lossy().into_owned());
    assert_eq!(
        settings["claudeManagedDir"],
        server.paths.claude_managed_projects().to_string_lossy().into_owned(),
        "the sidebar needs both roots to explain where a session came from"
    );
    assert_eq!(settings["codexDir"], server.paths.codex_sessions().to_string_lossy().into_owned());
    assert!(settings["serverStartedAt"].as_str().is_some_and(|s| s.ends_with('Z')));

    let snapshot = next_frame(&mut stream).await;
    assert_eq!(snapshot["type"], "sessions:snapshot");
    let sessions = snapshot["sessions"].as_array().expect("sessions array");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], SESSION_ID);
    assert_eq!(sessions[0]["title"], "watch me");

    // Appending changes the transcript's size and mtime, so its summary
    // reserializes differently — which is exactly what an upsert is.
    let transcript = server.home.join(format!(".claude/projects/-Users-me-proj/{SESSION_ID}.jsonl"));
    let mut file = std::fs::OpenOptions::new().append(true).open(&transcript).expect("open transcript");
    let rename = r#"{"type":"custom-title","customTitle":"renamed while watching","timestamp":"2026-05-01T10:00:05.000Z"}"#;
    writeln!(file, "{rename}").expect("append");
    file.flush().expect("flush");

    let delta = next_frame_of(&mut stream, "sessions:delta").await;
    let upserts = delta["upserts"].as_array().expect("upserts array");
    assert_eq!(upserts.len(), 1, "only the session that changed is sent");
    assert_eq!(upserts[0]["id"], SESSION_ID);
    assert_eq!(upserts[0]["title"], "renamed while watching");
    assert_eq!(delta["removed"].as_array().expect("removed array").len(), 0);

    server.stop().await;
}

/// `GET /api/settings` and the feed's opening frame describe the same server, so
/// a client that reads one and reconnects to the other cannot see them disagree.
/// The route adds the session counts; everything else must match field for field.
#[tokio::test]
async fn the_settings_route_and_the_feed_frame_agree() {
    let server = common::TestServer::start_with(seed_session).await;
    let (socket, _) = tokio_tungstenite::connect_async(&server.ws_url("/api/events"))
        .await
        .expect("websocket connect");
    let (_sink, mut stream) = socket.split();
    let frame = next_frame(&mut stream).await;

    let settings = server.get_json("/api/settings").await;
    for field in [
        // Which server this is, before anything about what it holds: a client
        // watching the feed has to be able to tell one instance from another.
        "instance",
        "hostname",
        "platform",
        "nodeVersion",
        "homeDir",
        "claudeDir",
        "claudeManagedDir",
        "codexDir",
    ] {
        assert_eq!(settings[field], frame[field], "{field} differs between the route and the feed");
    }
    assert_eq!(settings["serverStartedAt"], frame["serverStartedAt"]);
    assert_eq!(settings["claudeSessionCount"], 1, "only the route counts sessions");
    assert_eq!(settings["codexSessionCount"], 0);
    assert_eq!(
        settings["claudeManagedDir"],
        server.paths.claude_managed_projects().to_string_lossy().into_owned()
    );

    server.stop().await;
}

#[tokio::test]
async fn a_deleted_session_is_removed_by_key() {
    let server = common::TestServer::start_with(seed_session).await;
    let (socket, _) = tokio_tungstenite::connect_async(&server.ws_url("/api/events"))
        .await
        .expect("websocket connect");
    let (_sink, mut stream) = socket.split();

    assert_eq!(next_frame(&mut stream).await["type"], "settings");
    assert_eq!(next_frame(&mut stream).await["type"], "sessions:snapshot");

    let transcript = server.home.join(format!(".claude/projects/-Users-me-proj/{SESSION_ID}.jsonl"));
    std::fs::remove_file(&transcript).expect("remove transcript");

    let delta = next_frame_of(&mut stream, "sessions:delta").await;
    let removed = delta["removed"].as_array().expect("removed array");
    assert_eq!(removed.len(), 1);
    assert_eq!(
        removed[0], "claude/11111111-2222-3333-4444-555555555555",
        "the key is source/id, which is how the web client indexes its map"
    );

    server.stop().await;
}

#[tokio::test]
async fn the_watcher_stops_with_the_last_client() {
    let server = common::TestServer::start_with(seed_session).await;
    let (socket, _) = tokio_tungstenite::connect_async(&server.ws_url("/api/events"))
        .await
        .expect("websocket connect");
    let (mut sink, mut stream) = socket.split();
    assert_eq!(next_frame(&mut stream).await["type"], "settings");

    use futures_util::SinkExt;
    sink.close().await.expect("close");
    drop(stream);

    // Detaching happens on the server's socket task, so give it a moment.
    for _ in 0..50 {
        let (_, body) = server.get("/api/settings").await;
        assert!(!body.is_empty(), "the server keeps serving after a socket goes away");
        tokio::time::sleep(Duration::from_millis(50)).await;
        if !server.feed_is_running() {
            break;
        }
    }
    assert!(!server.feed_is_running(), "an idle server must hold no recursive filesystem watch");

    server.stop().await;
}
