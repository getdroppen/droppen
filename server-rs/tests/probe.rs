//! End-to-end proof of the four seams, against a synthetic `~/.claude` tree.
//!
//! Nothing here touches the real home directory: the server is pointed at a
//! tempdir through `RemoteHandApp::home_override`, which exists precisely so
//! discovery can be tested without depending on (or reading) a user's sessions.

use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use remote_hand_server::app::RemoteHandApp;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

const SESSION_ID: &str = "11111111-2222-3333-4444-555555555555";

/// A running server plus everything needed to talk to it and stop it.
struct Probe {
    addr: SocketAddr,
    cancel: CancellationToken,
    feeds: std::sync::Arc<remote_hand_server::feed::FeedRegistry>,
    transcript: PathBuf,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
    _home: tempfile::TempDir,
}

/// Give a transcript a definite mtime, since the session list is ordered by it
/// and two files written in the same instant would otherwise tie.
fn set_mtime(path: &Path, seconds_since_epoch: u64) {
    let when = std::time::UNIX_EPOCH + Duration::from_secs(seconds_since_epoch);
    let file = std::fs::OpenOptions::new().write(true).open(path).expect("open for times");
    file.set_times(std::fs::FileTimes::new().set_accessed(when).set_modified(when))
        .expect("set mtime");
}

impl Probe {
    async fn start() -> Probe {
        let home = tempfile::tempdir().expect("tempdir");
        let project = home.path().join(".claude/projects/-Users-me-proj");
        std::fs::create_dir_all(&project).expect("project dir");
        let transcript = project.join(format!("{SESSION_ID}.jsonl"));
        std::fs::write(
            &transcript,
            concat!(
                r#"{"type":"user","cwd":"/Users/me/proj","gitBranch":"main","timestamp":"2024-05-01T10:00:00.000Z","message":{"content":[{"type":"text","text":"port the server"}]}}"#,
                "\n",
                r#"{"type":"assistant","timestamp":"2024-05-01T10:00:01.000Z","message":{"model":"claude-x","content":[{"type":"text","text":"on it"}]}}"#,
                "\n",
            ),
        )
        .expect("seed transcript");

        // A second project, so the list proves it walks every project directory.
        let other = home.path().join(".claude/projects/-Users-me-other");
        std::fs::create_dir_all(&other).expect("other project dir");
        std::fs::write(
            other.join("66666666-7777-8888-9999-000000000000.jsonl"),
            format!(
                "{}\n",
                r#"{"type":"summary","summary":"other session","cwd":"/Users/me/other","timestamp":"2024-04-01T10:00:00.000Z"}"#
            ),
        )
        .expect("seed other transcript");

        set_mtime(&transcript, 1_714_500_000);
        set_mtime(&other.join("66666666-7777-8888-9999-000000000000.jsonl"), 1_711_900_000);

        let app = RemoteHandApp {
            home_override: Some(home.path().to_path_buf()),
            port_override: Some(0),
            // No login: these tests are not about authentication (`tests/auth.rs` is).
            auth_enabled_override: Some(false),
            ..Default::default()
        };
        let prepared = app.prepare().await.expect("prepare");
        let addr = prepared.addr;
        let cancel = prepared.cancel();
        let feeds = prepared.feeds();
        let server = tokio::spawn(prepared.serve_task());

        Probe {
            addr,
            cancel,
            feeds,
            transcript,
            server,
            _home: home,
        }
    }

    async fn get(&self, path: &str) -> (u16, String) {
        http_get(self.addr, path).await
    }

    async fn get_json(&self, path: &str) -> Value {
        let (status, body) = self.get(path).await;
        assert_eq!(status, 200, "GET {path} -> {status}: {body}");
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("GET {path} returned non-JSON ({e}): {body}"))
    }

    async fn stop(self) {
        self.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.server).await;
    }
}

/// A one-shot HTTP/1.1 GET, returning `(status, body)`.
async fn http_get(addr: SocketAddr, path: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read");
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").expect("malformed HTTP response");
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("status line");
    (status, body.to_string())
}

/// The read half of a connected feed socket.
type FeedStream = futures_util::stream::SplitStream<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>>;

/// The next frame, parsed. Ten seconds is generous on purpose: where the
/// platform's filesystem watcher is unavailable the 2 s poll backstop is what
/// delivers, and a loaded machine can miss a tick.
async fn next_frame(stream: &mut FeedStream) -> Value {
    let message = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("timed out waiting for a frame")
        .expect("stream ended")
        .expect("websocket error");
    serde_json::from_str::<Value>(&message.into_text().expect("text frame")).expect("json frame")
}

/// The next frame that is not a periodic `state` push.
async fn next_transcript_frame(stream: &mut FeedStream) -> Value {
    loop {
        let frame = next_frame(stream).await;
        if frame["type"] != "state" {
            return frame;
        }
    }
}

fn append(path: &Path, line: &str) {
    let mut file = std::fs::OpenOptions::new().append(true).open(path).expect("open transcript");
    writeln!(file, "{line}").expect("append");
    file.flush().expect("flush");
}

#[tokio::test]
async fn the_session_list_reflects_the_home_directory() {
    let probe = Probe::start().await;
    // These fixtures are the *user's* own `~/.claude`, which no server owns, so
    // the default list is empty and `scope=all` is what asks to see them.
    assert_eq!(probe.get_json("/api/sessions").await["total"], 0);
    let body = probe.get_json("/api/sessions?scope=all").await;

    assert_eq!(body["total"], 2);
    let sessions = body["sessions"].as_array().expect("sessions array");
    assert_eq!(sessions.len(), 2);
    // Newest first.
    assert_eq!(sessions[0]["id"], SESSION_ID);
    assert_eq!(sessions[0]["source"], "claude");
    assert_eq!(sessions[0]["scope"], "external", "the user's own root belongs to no server");
    assert_eq!(sessions[0]["title"], "port the server");
    assert_eq!(sessions[0]["cwd"], "/Users/me/proj");
    assert_eq!(sessions[0]["gitBranch"], "main");
    assert_eq!(sessions[0]["status"], "idle", "no live process is registered");
    assert_eq!(sessions[1]["title"], "other session");
    assert_eq!(sessions[1]["gitBranch"], Value::Null, "an absent branch is null, not missing");

    probe.stop().await;
}

#[tokio::test]
async fn the_session_list_filters_and_limits() {
    let probe = Probe::start().await;

    let by_query = probe.get_json("/api/sessions?q=OTHER&scope=all").await;
    assert_eq!(by_query["total"], 1, "the search is case-insensitive over title and cwd");
    assert_eq!(
        probe.get_json("/api/sessions?q=OTHER&scope=external").await["total"],
        1,
        "and it composes with the scope filter"
    );
    assert_eq!(by_query["sessions"][0]["title"], "other session");

    let by_source = probe.get_json("/api/sessions?source=codex&scope=all").await;
    assert_eq!(by_source["total"], 0, "codex discovery is stubbed out in this probe");

    let limited = probe.get_json("/api/sessions?limit=1&scope=all").await;
    assert_eq!(limited["total"], 2, "total counts matches, not the returned page");
    assert_eq!(limited["sessions"].as_array().expect("array").len(), 1);

    probe.stop().await;
}

#[tokio::test]
async fn a_session_detail_carries_its_transcript_and_a_missing_one_404s() {
    let probe = Probe::start().await;

    let detail = probe.get_json(&format!("/api/sessions/claude/{SESSION_ID}")).await;
    assert_eq!(detail["summary"]["id"], SESSION_ID);
    assert_eq!(detail["itemsChanged"], true);
    assert_eq!(detail["reset"], true);
    assert_eq!(detail["totalItems"], 2);
    assert_eq!(detail["items"][0]["role"], "user");
    assert_eq!(detail["items"][0]["text"], "port the server");
    assert_eq!(detail["items"][1]["role"], "assistant");
    assert_eq!(detail["injection"], Value::Null);
    assert_eq!(
        detail["canSend"], false,
        "an external session is readable in full and still not this server's to drive"
    );
    assert_eq!(detail["cliChoice"]["model"], "claude-x");

    // The cheap poll path: an unchanged signature returns no items.
    let sig = detail["sig"].as_str().expect("sig");
    let polled = probe.get_json(&format!("/api/sessions/claude/{SESSION_ID}?after=2&sig={sig}")).await;
    assert_eq!(polled["itemsChanged"], false);
    assert_eq!(polled["items"].as_array().expect("array").len(), 0);

    let (status, body) = probe.get("/api/sessions/claude/does-not-exist").await;
    assert_eq!(status, 404, "the multi-status response the OpenAPI registry documents");
    assert_eq!(serde_json::from_str::<Value>(&body).expect("json")["error"], "session not found");

    probe.stop().await;
}

#[tokio::test]
async fn the_spec_is_served_and_names_the_ported_routes() {
    let probe = Probe::start().await;
    let spec = probe.get_json("/openapi.json").await;

    assert_eq!(spec["info"]["title"], "remote-hand API");
    assert_eq!(spec["info"]["version"], "0.1.0");
    assert!(spec["paths"]["/api/sessions"].is_object());
    // The 404 the framework cannot express, added by `openapi_ext`.
    let detail = &spec["paths"]["/api/sessions/{source}/{id}"]["get"];
    assert!(detail["responses"]["404"].is_object(), "the extra-response registry did not apply");
    assert_eq!(detail["responses"]["default"], Value::Null, "the derived `default` should be replaced");

    // The committed contract is this document (`export-openapi` writes the
    // same one). A route or doc-comment change that skipped `pnpm gen:api`
    // leaves `openapi/openapi.json` behind, and `web/src/api/schema.d.ts`,
    // generated from it, with it.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../openapi/openapi.json");
    let text = std::fs::read_to_string(path).expect("read openapi/openapi.json");
    let committed: Value = serde_json::from_str(&text).expect("openapi/openapi.json is JSON");
    assert!(
        spec == committed,
        "openapi/openapi.json is not the spec this server serves (first difference at `{}`): \
         run `pnpm gen:api` and commit openapi/openapi.json and web/src/api/schema.d.ts",
        first_difference(&committed, &spec, String::new())
    );

    probe.stop().await;
}

/// Where two JSON documents first differ, as a JSON pointer.
fn first_difference(a: &Value, b: &Value, at: String) -> String {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for key in x.keys().chain(y.keys()) {
                match (x.get(key), y.get(key)) {
                    (Some(p), Some(q)) if p == q => {}
                    (Some(p), Some(q)) => return first_difference(p, q, format!("{at}/{key}")),
                    _ => return format!("{at}/{key}"),
                }
            }
            at
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            match x.iter().zip(y).position(|(p, q)| p != q) {
                Some(i) => first_difference(&x[i], &y[i], format!("{at}/{i}")),
                None => at,
            }
        }
        _ => at,
    }
}

#[tokio::test]
async fn a_non_api_path_falls_through_to_the_spa() {
    let probe = Probe::start().await;

    // Either the embedded shell (200) or the explanatory stub (503), depending
    // on whether `web/dist` existed at build time — never a bare 404.
    let (status, _) = probe.get("/sessions/claude/some-id").await;
    assert!(status == 200 || status == 503, "the SPA fallback returned {status}");

    // …but an unmatched API path stays a JSON 404.
    let (status, body) = probe.get("/api/nope").await;
    assert_eq!(status, 404);
    assert!(body.contains("\"error\""), "an API 404 must be JSON, got: {body}");

    probe.stop().await;
}

#[tokio::test]
async fn the_feed_sends_a_snapshot_then_a_delta_when_the_transcript_grows() {
    let probe = Probe::start().await;
    let url = format!("ws://{}/api/sessions/claude/{SESSION_ID}/ws", probe.addr);
    let (socket, _) = tokio_tungstenite::connect_async(&url).await.expect("websocket connect");
    let (_sink, mut stream) = socket.split();

    let snapshot = next_frame(&mut stream).await;
    assert_eq!(snapshot["type"], "snapshot");
    assert_eq!(snapshot["totalItems"], 2);
    assert_eq!(snapshot["startIndex"], 0);
    assert_eq!(snapshot["omittedItems"], 0);
    assert_eq!(snapshot["hasMoreBefore"], false);
    assert_eq!(snapshot["summary"]["id"], SESSION_ID, "the snapshot carries the state fields inline");
    assert_eq!(snapshot["items"][1]["text"], "on it");

    let state = next_frame(&mut stream).await;
    assert_eq!(state["type"], "state", "the state replay closes the attach race");
    assert_eq!(state["summary"]["id"], SESSION_ID);

    append(
        &probe.transcript,
        r#"{"type":"assistant","timestamp":"2024-05-01T10:00:02.000Z","message":{"model":"claude-x","content":[{"type":"text","text":"appended"}]}}"#,
    );

    // The filesystem watcher usually gets there first; the 2 s poll backstop
    // covers the environments where it does not.
    let delta = next_transcript_frame(&mut stream).await;
    assert_eq!(delta["type"], "delta", "an append after an unchanged prefix must not force a reset");
    assert_eq!(delta["fromIndex"], 2);
    assert_eq!(delta["totalItems"], 3);
    assert_eq!(delta["previousSig"], snapshot["sig"]);
    assert_eq!(delta["items"][0]["text"], "appended");

    probe.stop().await;
}

#[tokio::test]
async fn a_rewritten_transcript_replaces_from_the_first_changed_item() {
    let probe = Probe::start().await;
    let url = format!("ws://{}/api/sessions/claude/{SESSION_ID}/ws", probe.addr);
    let (socket, _) = tokio_tungstenite::connect_async(&url).await.expect("websocket connect");
    let (_sink, mut stream) = socket.split();

    let snapshot = next_frame(&mut stream).await;
    assert_eq!(snapshot["type"], "snapshot");
    assert_eq!(next_frame(&mut stream).await["type"], "state");

    // A live delta replaces from the first changed item. With no matching
    // items, it must replace from zero rather than append to the old transcript.
    std::fs::write(
        &probe.transcript,
        format!(
            "{}\n",
            r#"{"type":"user","cwd":"/Users/me/proj","timestamp":"2024-05-02T10:00:00.000Z","message":{"content":[{"type":"text","text":"rewritten"}]}}"#
        ),
    )
    .expect("rewrite");

    let frame = next_transcript_frame(&mut stream).await;
    assert_eq!(frame["type"], "delta");
    assert_eq!(frame["fromIndex"], 0, "the changed prefix must be replaced, not appended to");
    assert_eq!(frame["previousSig"], snapshot["sig"]);
    assert_eq!(frame["endIndex"], 1);
    assert_eq!(frame["totalItems"], 1);
    assert_eq!(frame["items"].as_array().expect("items").len(), 1);
    assert_eq!(frame["items"][0]["text"], "rewritten");

    probe.stop().await;
}

#[tokio::test]
async fn cancelling_stops_the_server_and_tears_down_the_feed_watchers() {
    let probe = Probe::start().await;
    let url = format!("ws://{}/api/sessions/claude/{SESSION_ID}/ws", probe.addr);
    let (socket, _) = tokio_tungstenite::connect_async(&url).await.expect("websocket connect");
    let (_sink, mut stream) = socket.split();
    // Drain the opening frames so the feed is genuinely live before shutdown.
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.next()).await.expect("snapshot");

    let addr = probe.addr;
    let feeds = probe.feeds.clone();
    probe.cancel.cancel();

    // The socket is told the feed is closing rather than being dropped on the floor.
    let mut sawrelevant = false;
    while let Ok(Some(Ok(message))) = tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
        if let Ok(text) = message.into_text() {
            if text.contains("\"closed\"") {
                sawrelevant = true;
                break;
            }
        }
    }
    assert!(sawrelevant, "a live socket must be told the server is going away");

    let stopped = tokio::time::timeout(Duration::from_secs(5), probe.server).await;
    assert!(stopped.is_ok(), "the server task must finish after cancellation");
    // The socket task's `FeedAttachment` drops as the socket closes, which is
    // what unregisters the watcher.
    for _ in 0..50 {
        if feeds.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(feeds.len(), 0, "no feed watcher should be left registered");

    // The listener is gone.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(tokio::net::TcpStream::connect(addr).await.is_err(), "the port must be released");
}

/// `z.coerce.number().int().min(1).max(1000)` was a `400` there, and clamping
/// here would quietly answer `limit=0` with two hundred rows instead.
#[tokio::test]
async fn a_limit_outside_the_documented_range_is_a_400() {
    let probe = Probe::start().await;

    for limit in ["0", "1001", "99999"] {
        let (status, body) = probe.get(&format!("/api/sessions?limit={limit}")).await;
        assert_eq!(status, 400, "limit={limit} -> {status}: {body}");
        let value: serde_json::Value = serde_json::from_str(&body).expect("json");
        assert_eq!(value["error"], "limit must be between 1 and 1000");
    }

    // The ends of the range are inside it.
    for limit in ["1", "1000"] {
        let (status, body) = probe.get(&format!("/api/sessions?limit={limit}")).await;
        assert_eq!(status, 200, "limit={limit} -> {status}: {body}");
    }

    probe.stop().await;
}
