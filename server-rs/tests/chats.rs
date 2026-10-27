//! Instant chats end to end, against a provider the test owns.
//!
//! The BYO-API backend is the one subsystem whose behaviour is defined by a
//! *third* party, so the fixture below stands in for it: a listener on an
//! ephemeral port that records what it was sent and replies from a script. The
//! server is pointed at it through the ordinary `PUT /api/chat-provider` route,
//! which means the tests exercise the real configuration path and never reach
//! the network.
//!
//! Two contracts are pinned here that no other test can see:
//!
//! * the **SSE frames** the browser reads. `POST /api/api-chats/{id}/message`
//!   is deliberately not an OpenAPI route, so `contract-diff` says nothing about
//!   it; `web/src/features/chat/api-chat-view.tsx:15-57` is the only other
//!   statement of the shape, and these tests are the other half.
//! * the **API key never leaving the server**, asserted over HTTP rather than
//!   over the store, because that is where a leak would actually happen.
//!
//! The CLI-backed half needs the fake `claude` on `PATH` and lives in
//! `tests/chats_cli.rs`, in its own binary so the two fixtures stay separate.

#[path = "common/mod.rs"]
mod common;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const KEY: &str = "sk-test-SECRETVALUE-0123456789";

// ------------------------------------------------------------- the fake provider

/// One request the fake provider received.
struct Received {
    path: String,
    authorization: String,
    body: String,
}

/// What the fake provider answers with.
enum Reply {
    /// A complete body under a status.
    Body(u16, String),
    /// Raw bytes written in pieces, so a chunk boundary can be put anywhere —
    /// including inside a multi-byte character.
    Pieces(Vec<Vec<u8>>),
    /// A stream that never ends on its own, and flips the flag when the reader
    /// goes away. How "the turn was abandoned" is observed from the far side.
    Endless(Arc<AtomicBool>),
}

/// How many frames the endless stream gives up after, so a server that never
/// hangs up fails the test rather than hanging it.
const ENDLESS_CAP: usize = 2_000;

/// An OpenAI-compatible endpoint the test owns end to end.
struct FakeProvider {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Received>>>,
}

impl FakeProvider {
    /// Start answering, routing on the request path.
    async fn start(reply: impl Fn(&str) -> Reply + Send + Sync + 'static) -> FakeProvider {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let reply = Arc::new(reply);
        tokio::spawn({
            let seen = seen.clone();
            async move {
                while let Ok((socket, _)) = listener.accept().await {
                    let seen = seen.clone();
                    let reply = reply.clone();
                    tokio::spawn(async move { serve(socket, seen, reply).await });
                }
            }
        });
        FakeProvider { addr, seen }
    }

    /// The `baseUrl` to configure. The `/v1` suffix is what a real provider
    /// carries, and proves the server appends rather than replaces.
    fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    fn received(&self) -> std::sync::MutexGuard<'_, Vec<Received>> {
        self.seen.lock().expect("provider log poisoned")
    }
}

async fn serve(mut socket: TcpStream, seen: Arc<Mutex<Vec<Received>>>, reply: Arc<impl Fn(&str) -> Reply>) {
    let Some(request) = read_request(&mut socket).await else { return };
    let answer = reply(&request.path);
    seen.lock().expect("provider log poisoned").push(request);
    match answer {
        Reply::Body(status, body) => {
            let head = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.write_all(body.as_bytes()).await;
        }
        Reply::Pieces(pieces) => {
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            let _ = socket.write_all(head.as_bytes()).await;
            for piece in pieces {
                let _ = socket.write_all(&piece).await;
                let _ = socket.flush().await;
                // Long enough that the pieces reach the server as separate
                // reads, which is the point of splitting them.
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        }
        Reply::Endless(hung_up) => {
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            let _ = socket.write_all(head.as_bytes()).await;
            for _ in 0..ENDLESS_CAP {
                if socket.write_all(&completion_frame("tick")).await.is_err() {
                    hung_up.store(true, Ordering::SeqCst);
                    return;
                }
                let _ = socket.flush().await;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
    let _ = socket.shutdown().await;
}

/// Read one HTTP/1.1 request: the head, then exactly `Content-Length` bytes.
async fn read_request(socket: &mut TcpStream) -> Option<Received> {
    let mut raw = Vec::new();
    let mut buffer = [0u8; 4096];
    let head_end = loop {
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        raw.extend_from_slice(&buffer[..read]);
        if let Some(index) = find(&raw, b"\r\n\r\n") {
            break index + 4;
        }
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let path = head.lines().next()?.split_whitespace().nth(1)?.to_string();
    let header = |name: &str| {
        head.lines()
            .find(|line| line.to_lowercase().starts_with(&format!("{name}:")))
            .and_then(|line| line.split_once(':'))
            .map(|(_, value)| value.trim().to_string())
    };
    let length: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = raw[head_end..].to_vec();
    while body.len() < length {
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    Some(Received {
        path,
        authorization: header("authorization").unwrap_or_default(),
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// One `data:` frame of a provider stream.
fn completion_frame(content: &str) -> Vec<u8> {
    format!("data: {}\n\n", json!({ "choices": [{ "delta": { "content": content } }] })).into_bytes()
}

// ------------------------------------------------------------------ the fixture

/// A server whose provider is the fake one, already configured.
async fn configured(provider: &FakeProvider) -> common::TestServer {
    let server = common::TestServer::start().await;
    let (status, body) = server
        .json(
            "PUT",
            "/api/chat-provider",
            &json!({ "baseUrl": provider.base_url(), "apiKey": KEY, "model": "gpt-4o" }),
        )
        .await;
    assert_eq!(status, 200, "configure the provider: {body}");
    server
}

/// Create an API-backed chat and return its id.
async fn new_api_chat(server: &common::TestServer) -> String {
    let (status, body) = server.json("POST", "/api/chats/new", &json!({ "backend": "api" })).await;
    assert_eq!(status, 200, "new chat: {body}");
    let value: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(value["kind"], "api");
    value["id"].as_str().expect("id").to_string()
}

/// Every `data:` payload of an SSE body, parsed. Keep-alive comments and blank
/// lines are skipped exactly as the browser skips them.
fn frames(body: &str) -> Vec<Value> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|payload| serde_json::from_str(payload.trim()).ok())
        .collect()
}

/// Send a message and return the frames the client saw.
async fn stream_turn(server: &common::TestServer, id: &str, text: &str) -> Vec<Value> {
    let (status, body) = server
        .json("POST", &format!("/api/api-chats/{id}/message"), &json!({ "text": text }))
        .await;
    assert_eq!(status, 200, "the stream itself is a 200: {body}");
    frames(&body)
}

// -------------------------------------------------------------------- the tests

#[tokio::test]
async fn the_api_key_is_stored_but_never_returned() {
    let provider = FakeProvider::start(|_| Reply::Body(200, "{}".into())).await;
    let server = configured(&provider).await;

    let (status, raw) = server.get("/api/chat-provider").await;
    assert_eq!(status, 200);
    assert!(!raw.contains(KEY), "the key must never appear in a response: {raw}");
    assert_eq!(
        serde_json::from_str::<Value>(&raw).expect("json"),
        json!({ "baseUrl": provider.base_url(), "model": "gpt-4o", "hasKey": true }),
    );

    // …and the same after a patch that does not mention the key at all.
    let (_, raw) = server.json("PUT", "/api/chat-provider", &json!({ "model": "gpt-4o-mini" })).await;
    assert!(!raw.contains(KEY), "a patch response must not leak it either: {raw}");
    assert_eq!(serde_json::from_str::<Value>(&raw).expect("json")["hasKey"], true);

    // It is only ever spent on the provider's `Authorization` header.
    let (status, _) = server.get("/api/chat-provider/models").await;
    assert_eq!(status, 200);
    assert_eq!(provider.received().last().expect("a call").authorization, format!("Bearer {KEY}"));
    server.stop().await;
}

#[tokio::test]
async fn the_models_route_asks_the_provider_and_sorts_what_it_gets() {
    let provider = FakeProvider::start(|path| {
        assert_eq!(path, "/v1/models", "the server appends to the configured baseUrl");
        Reply::Body(200, json!({ "data": [{ "id": "zeta" }, { "id": "alpha" }, { "no": "id" }] }).to_string())
    })
    .await;
    let server = configured(&provider).await;

    assert_eq!(server.get_json("/api/chat-provider/models").await, json!({ "models": ["alpha", "zeta"] }));
    server.stop().await;
}

#[tokio::test]
async fn a_provider_that_refuses_the_model_list_is_a_400() {
    let provider = FakeProvider::start(|_| Reply::Body(503, "nope".into())).await;
    let server = configured(&provider).await;

    let (status, body) = server.get("/api/chat-provider/models").await;
    assert_eq!(status, 400);
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("json"),
        json!({ "error": "provider /models failed: HTTP 503" })
    );
    server.stop().await;
}

#[tokio::test]
async fn an_unconfigured_provider_says_which_field_is_missing() {
    let server = common::TestServer::start().await;

    for path in ["/api/chat-provider/models"] {
        let (status, body) = server.get(path).await;
        assert_eq!(status, 400, "{path}");
        assert_eq!(
            serde_json::from_str::<Value>(&body).expect("json")["error"],
            "no API base URL configured — set it in settings"
        );
    }
    let (status, body) = server.json("POST", "/api/chats/new", &json!({ "backend": "api" })).await;
    assert_eq!(status, 400);
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("json")["error"],
        "no API base URL configured — set it in settings"
    );
    server.stop().await;
}

#[tokio::test]
async fn a_turn_streams_deltas_then_done_and_stores_both_messages() {
    // "你好" is split across two writes *inside* the first character, which a
    // string-buffered reassembler would turn into replacement characters.
    let greeting = completion_frame("你好");
    let split = greeting.iter().position(|&b| b == 0xe4).expect("the first byte of 你") + 1;
    let provider = FakeProvider::start(move |path| {
        assert_eq!(path, "/v1/chat/completions");
        Reply::Pieces(vec![
            completion_frame("Hel"),
            greeting[..split].to_vec(),
            greeting[split..].to_vec(),
            // Everything a provider interleaves with the content.
            b": keep-alive\n\n".to_vec(),
            completion_frame(""),
            b"data: [DONE]\n\n".to_vec(),
        ])
    })
    .await;
    let server = configured(&provider).await;
    let id = new_api_chat(&server).await;

    let frames = stream_turn(&server, &id, "hi there").await;
    let deltas: Vec<&str> = frames
        .iter()
        .filter(|f| f["type"] == "delta")
        .map(|f| f["text"].as_str().expect("text"))
        .collect();
    assert_eq!(deltas, vec!["Hel", "你好"], "one frame per non-empty content chunk");

    let done = frames.last().expect("a closing frame");
    assert_eq!(done["type"], "done");
    assert_eq!(done["content"], "Hel你好", "the closing frame carries the whole reply");

    // What the provider was asked for.
    let sent: Value = serde_json::from_str(&provider.received().last().expect("a call").body).expect("json");
    assert_eq!(sent["model"], "gpt-4o");
    assert_eq!(sent["stream"], true);
    assert_eq!(sent["messages"], json!([{ "role": "user", "content": "hi there" }]));

    // What was stored.
    let chat = server.get_json(&format!("/api/api-chats/{id}")).await;
    assert_eq!(chat["title"], "hi there", "the first message names the chat");
    assert_eq!(chat["messages"][0]["role"], "user");
    assert_eq!(chat["messages"][0]["content"], "hi there");
    assert_eq!(chat["messages"][1]["role"], "assistant");
    assert_eq!(chat["messages"][1]["content"], "Hel你好");
    assert!(chat["updatedAt"].as_str().expect("updatedAt") >= chat["createdAt"].as_str().expect("createdAt"));
    server.stop().await;
}

#[tokio::test]
async fn a_provider_failure_becomes_an_error_frame_and_leaves_the_user_turn() {
    let provider = FakeProvider::start(|_| Reply::Body(429, "slow down".into())).await;
    let server = configured(&provider).await;
    let id = new_api_chat(&server).await;

    let frames = stream_turn(&server, &id, "hello").await;
    assert_eq!(frames.len(), 1, "no deltas, just the failure: {frames:?}");
    assert_eq!(frames[0]["type"], "error");
    assert_eq!(
        frames[0]["error"], "provider request failed: HTTP 429 — slow down",
        "the provider's own words are quoted back"
    );

    // The user's message survives a failed turn: it was persisted first.
    let chat = server.get_json(&format!("/api/api-chats/{id}")).await;
    assert_eq!(chat["messages"].as_array().expect("messages").len(), 1);
    assert_eq!(chat["messages"][0]["role"], "user");
    server.stop().await;
}

#[tokio::test]
async fn a_message_to_a_chat_that_does_not_exist_is_an_error_frame() {
    let provider = FakeProvider::start(|_| Reply::Body(200, "{}".into())).await;
    let server = configured(&provider).await;

    // A well-formed segment that names nothing, and one the id pattern refuses.
    for id in ["missing", "bad.id"] {
        let frames = stream_turn(&server, id, "hello").await;
        assert_eq!(frames[0]["type"], "error");
        assert_eq!(frames[0]["error"], "chat not found", "{id}");
    }
    assert!(provider.received().is_empty(), "an unknown chat never reaches the provider");
    server.stop().await;
}

#[tokio::test]
async fn an_unusable_send_body_is_a_flat_400() {
    let provider = FakeProvider::start(|_| Reply::Body(200, "{}".into())).await;
    let server = configured(&provider).await;
    let id = new_api_chat(&server).await;

    for body in [
        json!({}),
        json!({ "text": "" }),
        json!({ "text": "hi", "model": "--dangerous" }),
        json!(null),
    ] {
        let (status, raw) = server.json("POST", &format!("/api/api-chats/{id}/message"), &body).await;
        assert_eq!(status, 400, "{body}");
        assert_eq!(
            serde_json::from_str::<Value>(&raw).expect("json"),
            json!({ "error": "invalid body" }),
            "{body}"
        );
    }
    server.stop().await;
}

#[tokio::test]
async fn a_chat_can_be_read_deleted_and_only_deleted_once() {
    let provider = FakeProvider::start(|_| Reply::Body(200, "{}".into())).await;
    let server = configured(&provider).await;
    let id = new_api_chat(&server).await;

    let chat = server.get_json(&format!("/api/api-chats/{id}")).await;
    assert_eq!(chat["id"], id.as_str());
    assert_eq!(chat["title"], "new chat");
    assert_eq!(chat["model"], "gpt-4o", "an empty request falls back to the configured default");
    assert_eq!(chat["messages"], json!([]));

    let (status, body) = server.json("DELETE", &format!("/api/api-chats/{id}"), &json!(null)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(serde_json::from_str::<Value>(&body).expect("json"), json!({ "accepted": true }));

    for (method, path) in [("GET", format!("/api/api-chats/{id}")), ("DELETE", format!("/api/api-chats/{id}"))] {
        let (status, body) = server.json(method, &path, &json!(null)).await;
        assert_eq!(status, 404, "{method} {path}");
        assert_eq!(serde_json::from_str::<Value>(&body).expect("json"), json!({ "error": "chat not found" }));
    }
    server.stop().await;
}

#[tokio::test]
async fn the_list_carries_both_backends_newest_first() {
    let provider = FakeProvider::start(|_| Reply::Body(200, "{}".into())).await;
    let server = common::TestServer::start().await;

    // A claude session anchored in the chats directory is a chat; one anywhere
    // else is not.
    let anchor = server.home.join(".local/share/remote-hand/chats");
    std::fs::create_dir_all(&anchor).expect("anchor");
    seed_claude_session(&server.home, "11111111-1111-4111-8111-111111111111", &anchor);
    seed_claude_session(&server.home, "22222222-2222-4222-8222-222222222222", &server.home.join("elsewhere"));

    let (status, body) = server
        .json(
            "PUT",
            "/api/chat-provider",
            &json!({ "baseUrl": provider.base_url(), "apiKey": KEY, "model": "gpt-4o" }),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let api_id = new_api_chat(&server).await;

    let list = server.get_json("/api/chats").await;
    let chats = list["chats"].as_array().expect("chats");
    let ids: Vec<&str> = chats.iter().map(|c| c["id"].as_str().expect("id")).collect();
    assert!(ids.contains(&api_id.as_str()), "the API chat is listed: {list}");
    assert!(
        ids.contains(&"11111111-1111-4111-8111-111111111111"),
        "the anchored session is a chat: {list}"
    );
    assert!(
        !ids.contains(&"22222222-2222-4222-8222-222222222222"),
        "a session elsewhere is not: {list}"
    );

    let cli = chats.iter().find(|c| c["kind"] == "claude").expect("the claude chat");
    assert_eq!(cli["model"], Value::Null, "remote-hand does not know what a CLI chat runs");
    let api = chats.iter().find(|c| c["kind"] == "api").expect("the api chat");
    assert_eq!(api["model"], "gpt-4o");

    let updated: Vec<&str> = chats.iter().map(|c| c["updatedAt"].as_str().expect("updatedAt")).collect();
    let mut sorted = updated.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(updated, sorted, "newest first");

    // The chat's session stays out of the ordinary session list.
    let sessions = server.get_json("/api/sessions").await;
    let session_ids: Vec<&str> = sessions["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .map(|s| s["id"].as_str().expect("id"))
        .collect();
    assert!(!session_ids.contains(&"11111111-1111-4111-8111-111111111111"), "a chat is not a session");
    server.stop().await;
}

/// Write a one-entry claude transcript whose `cwd` is `cwd`.
fn seed_claude_session(home: &std::path::Path, id: &str, cwd: &std::path::Path) {
    common::seed_transcript(
        &home.join(".claude"),
        "-seeded",
        id,
        &[json!({
            "type": "user",
            "cwd": cwd.to_string_lossy(),
            "timestamp": "2024-05-01T10:00:00.000Z",
            "message": { "content": [{ "type": "text", "text": "hello" }] },
        })],
    );
}

#[tokio::test]
async fn a_cli_chat_needs_a_first_message() {
    let server = common::TestServer::start().await;
    for body in [json!({ "backend": "claude" }), json!({ "backend": "codex", "text": "   " })] {
        let (status, raw) = server.json("POST", "/api/chats/new", &body).await;
        assert_eq!(status, 400, "{body}");
        assert_eq!(
            serde_json::from_str::<Value>(&raw).expect("json"),
            json!({ "error": "text is required for CLI chats" })
        );
    }
    server.stop().await;
}

#[tokio::test]
async fn hanging_up_mid_stream_abandons_the_provider_request() {
    // The provider streams until its reader goes away, and says so when it does.
    let hung_up = Arc::new(AtomicBool::new(false));
    let provider = FakeProvider::start({
        let hung_up = hung_up.clone();
        move |_| Reply::Endless(hung_up.clone())
    })
    .await;
    let server = configured(&provider).await;
    let id = new_api_chat(&server).await;

    // A raw connection, so it can be torn down mid-response — which is what a
    // browser tab being closed looks like from here.
    let mut socket = TcpStream::connect(server.addr).await.expect("connect");
    let body = json!({ "text": "hello" }).to_string();
    let request = format!(
        "POST /api/api-chats/{id}/message HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        server.addr,
        body.len()
    );
    socket.write_all(request.as_bytes()).await.expect("write");

    // Read until the first delta has actually been delivered, so the turn is
    // demonstrably under way before the connection goes.
    let mut seen = Vec::new();
    let mut buffer = [0u8; 1024];
    while !String::from_utf8_lossy(&seen).contains(r#""delta""#) {
        let read = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buffer))
            .await
            .expect("a first delta")
            .expect("read");
        assert!(read > 0, "the server closed before streaming anything");
        seen.extend_from_slice(&buffer[..read]);
    }
    drop(socket);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !hung_up.load(Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "the provider request outlived the client that asked for it"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Nothing was stored for a reply nobody is listening to; the user's own
    // message, written before the request, is still there.
    let chat = server.get_json(&format!("/api/api-chats/{id}")).await;
    assert_eq!(chat["messages"].as_array().expect("messages").len(), 1);
    assert_eq!(chat["messages"][0]["role"], "user");
    server.stop().await;
}
