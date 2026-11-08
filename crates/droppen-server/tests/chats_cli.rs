//! A CLI-backed instant chat, from the route down to the argv.
//!
//! Separate from `tests/chats.rs` because this half needs the fake CLIs on
//! `PATH` (`tests/support`) while that half needs a fake HTTP provider, and the
//! two fixtures have nothing to say to each other.
//!
//! What is asserted is the whole reason a chat is not just a session: the spawn
//! happens in the anchor directory, with the coding persona replaced and its
//! one tool — Read — left for the attachments a message carries.

mod support;

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Write the transcript a real claude writes by the time a second message can
/// be sent; the fake CLI writes none, and the send routes find their session in
/// the transcripts.
fn seed_claude_chat(probe: &support::Probe, id: &str, anchor: &Path) {
    let paths = droppen_instance::paths::Paths::new(probe.home.clone());
    let project = droppen_provider_claude_code::claude_roots::ClaudeRoots::of(&paths).managed_projects().join("-chats");
    std::fs::create_dir_all(&project).expect("project dir");
    let line = json!({
        "type": "user", "sessionId": id, "cwd": anchor.to_string_lossy(), "uuid": "u1",
        "timestamp": "2026-10-01T10:00:00Z", "message": {"role": "user", "content": "hi"},
    });
    std::fs::write(project.join(format!("{id}.jsonl")), format!("{line}\n")).expect("transcript");
}

/// The same for codex: its session list reads rollouts, and a rollout written
/// by the fake app-server is what makes the thread sendable.
async fn seed_codex_chat(probe: &support::Probe, id: &str, anchor: &Path) {
    let dir = probe.home.join(".codex/sessions/2026/09/11");
    std::fs::create_dir_all(&dir).expect("codex sessions dir");
    let rollout = dir.join(format!("rollout-2026-09-11T10-00-00-{id}.jsonl"));
    let meta = json!({"type": "session_meta", "timestamp": "2026-09-11T10:00:00.000Z", "payload": {"id": id, "cwd": anchor.to_string_lossy()}});
    std::fs::write(&rollout, format!("{meta}\n")).expect("rollout");
    let paths = droppen_instance::paths::Paths::new(probe.home.clone());
    droppen_provider_codex::sessions::codex::CodexSessions.record_owned(&paths, id)
        .await
        .expect("own the codex chat");
}

/// Wait until the chat is discoverable — the transcript a real CLI would have
/// written is what the message route resolves its id against.
async fn wait_for_chat(probe: &support::Probe, id: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let chats = probe.get_json("/api/chats").await;
        if chats["chats"].as_array().is_some_and(|list| list.iter().any(|chat| chat["id"] == id)) {
            return;
        }
        assert!(Instant::now() < deadline, "chat {id} was never listed: {chats}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_claude_chat_spawns_in_the_anchor_directory_with_the_persona() {
    let probe = support::Probe::start().await;
    let anchor = probe.data_dir().join("chats");

    let result = probe
        .send_ok("POST", "/api/chats/new", json!({ "backend": "claude", "text": "hi" }), 200)
        .await;
    assert_eq!(result["kind"], "claude");
    assert!(result["id"].as_str().expect("id").len() == 36, "a fresh session uuid: {result}");

    let spawn = probe.wait_for_spawn(&anchor).await;
    assert!(spawn.is_resident(), "a chat is a resident like any other session: {:?}", spawn.argv);
    assert_eq!(
        support::wait_for_prompts(&anchor, 1).await,
        vec!["hi".to_string()],
        "the prompt reaches the CLI over its stdin, not in its argv"
    );
    assert_eq!(
        spawn.flag("--tools"),
        Some("Read"),
        "a chat has one tool — Read, which is how it views an attachment"
    );
    assert_eq!(spawn.flag("--disallowedTools"), None, "and every other tool is gone by omission");
    assert!(
        spawn
            .flag("--system-prompt")
            .expect("a persona")
            .starts_with("You are a friendly general-purpose assistant in a casual chat app."),
        "the chat persona replaces the coding one"
    );

    // codex has no persona flag, so the anchor directory carries one for it.
    let persona = std::fs::read_to_string(anchor.join("AGENTS.md")).expect("AGENTS.md");
    assert!(persona.starts_with("# Instant chat\n"), "{persona}");
    assert!(persona.contains("NOT a coding session"), "{persona}");
    probe.stop().await;
}

/// A chat, too, can carry an account the start rule named (`accountFrom:
/// "start"`): checked again at the spawn, and placed afresh when the rule no
/// longer names it — never billed to an account only a stale answer named.
/// The same id without the marker is a person's pick.
#[tokio::test]
async fn a_chat_with_an_account_the_start_rule_named_is_checked_again_at_the_spawn() {
    let probe = support::Probe::start().await;
    let anchor = probe.data_dir().join("chats");
    let create = |label: &'static str, secret: &'static str| {
        let probe = &probe;
        async move { probe.create_account_id(label, secret).await }
    };
    const DEFAULT_SECRET: &str = "sk-ant-oat01-TESTCHATDEFAULT0123456789";
    const SPARE_SECRET: &str = "sk-ant-oat01-TESTCHATSPARE00123456789";
    let default = create("Default", DEFAULT_SECRET).await;
    let spare = create("Spare", SPARE_SECRET).await;
    probe.send_ok("PUT", "/api/accounts/default", json!({"accountId": default}), 200).await;

    // A stale answer named the spare: the rule places a chat on the default now.
    probe
        .send_ok(
            "POST",
            "/api/chats/new",
            json!({"backend": "claude", "text": "hi", "accountId": spare, "accountFrom": "start"}),
            200,
        )
        .await;
    let stale = probe.wait_for_spawn_number(&anchor, 1).await;
    assert_eq!(
        stale.env("CLAUDE_CODE_OAUTH_TOKEN"),
        Some(DEFAULT_SECRET),
        "placed afresh, on the default — not on the account the stale answer named"
    );

    // The same id as a person's pick: theirs.
    probe
        .send_ok("POST", "/api/chats/new", json!({"backend": "claude", "text": "hi", "accountId": spare}), 200)
        .await;
    let picked = probe.wait_for_spawn_number(&anchor, 2).await;
    assert_eq!(picked.env("CLAUDE_CODE_OAUTH_TOKEN"), Some(SPARE_SECRET));

    // A malformed marker is a bad request, not a pick.
    let (status, _) = probe
        .send("POST", "/api/chats/new", json!({"backend": "claude", "text": "hi", "accountId": spare, "accountFrom": "guess"}))
        .await;
    assert_eq!(status, 400);
    probe.stop().await;
}

/// A claude chat's first message carries its attachments exactly as a
/// session's does: the uploaded paths are appended to the prompt the CLI reads
/// over stdin, and the process has the one tool that can open them. A later
/// message carries its own the same way.
#[tokio::test]
async fn a_claude_chat_hands_its_attachments_to_the_cli() {
    let probe = support::Probe::start().await;
    let anchor = probe.data_dir().join("chats");
    let image = probe.upload("shot.png", &[0x89, b'P', b'N', b'G']).await["path"]
        .as_str()
        .expect("the image path")
        .to_string();
    let file = probe.upload("notes.txt", b"the notes").await["path"]
        .as_str()
        .expect("the file path")
        .to_string();

    let created = probe
        .send_ok(
            "POST",
            "/api/chats/new",
            json!({"backend": "claude", "text": "look", "images": [image], "files": [file]}),
            200,
        )
        .await;
    let id = created["id"].as_str().expect("the chat id").to_string();

    let spawn = probe.wait_for_spawn(&anchor).await;
    assert_eq!(spawn.flag("--tools"), Some("Read"), "the one tool an attachment is viewed with");
    assert_eq!(spawn.flag("--disallowedTools"), None, "and no tool beyond it");

    let prompt = support::wait_for_prompts(&anchor, 1).await.remove(0);
    assert_eq!(
        prompt,
        format!("look\n\n[2 attached file(s) — view with the Read tool:]\n- {image}\n- {file}"),
        "the same suffix a session's first message gets"
    );

    // And a follow-up message, in the same chat, carries the same way.
    seed_claude_chat(&probe, &id, &anchor);
    wait_for_chat(&probe, &id).await;
    let later = probe.upload("later.png", &[0x89, b'P', b'N', b'G']).await["path"]
        .as_str()
        .expect("the second image path")
        .to_string();
    probe
        .send_ok(
            "POST",
            &format!("/api/sessions/claude/{id}/message"),
            json!({"text": "and this", "images": [later]}),
            200,
        )
        .await;
    let prompts = support::wait_for_prompts(&anchor, 2).await;
    assert_eq!(
        prompts[1],
        format!("and this\n\n[1 attached file(s) — view with the Read tool:]\n- {later}"),
        "a chat's follow-up takes attachments exactly as a session's does"
    );
    probe.stop().await;
}

/// A codex chat takes images as app-server input and non-images as a path in
/// the prompt, and its thread is started with the chat persona — the same
/// delivery a session gets, on the same resident process.
#[tokio::test]
async fn a_codex_chat_hands_its_attachments_to_the_cli() {
    let probe = support::Probe::start().await;
    let anchor = probe.data_dir().join("chats");
    let image = probe.upload("shot.png", &[0x89, b'P', b'N', b'G']).await["path"]
        .as_str()
        .expect("the image path")
        .to_string();
    let file = probe.upload("notes.txt", b"the notes").await["path"]
        .as_str()
        .expect("the file path")
        .to_string();

    let created = probe
        .send_ok(
            "POST",
            "/api/chats/new",
            json!({"backend": "codex", "text": "look", "images": [image], "files": [file]}),
            200,
        )
        .await;
    let id = created["id"].as_str().expect("the chat id").to_string();

    let lines = support::wait_for_codex_stdin(&anchor, 4).await;
    let thread: Value = serde_json::from_str(&lines[2]).expect("thread/start");
    assert_eq!(thread["method"], "thread/start");
    assert!(
        thread["params"]["baseInstructions"].as_str().unwrap_or_default().contains("friendly general-purpose assistant"),
        "the chat persona is the thread's instructions: {}",
        lines[2]
    );
    let turn: Value = serde_json::from_str(&lines[3]).expect("turn/start");
    assert_eq!(turn["method"], "turn/start");
    let input = turn["params"]["input"].as_array().expect("input");
    assert_eq!(
        input[0]["text"],
        format!("look\n\n[1 attached file(s) — view with the Read tool:]\n- {file}"),
        "non-images travel in the prompt, as for a session"
    );
    assert_eq!(input[1], json!({"type": "localImage", "path": image}), "the image is input, not a path to read");

    // A follow-up in the same chat is another turn with its own attachments.
    seed_codex_chat(&probe, &id, &anchor).await;
    wait_for_chat(&probe, &id).await;
    let later = probe.upload("later.png", &[0x89, b'P', b'N', b'G']).await["path"]
        .as_str()
        .expect("the second image path")
        .to_string();
    probe
        .send_ok(
            "POST",
            &format!("/api/sessions/codex/{id}/message"),
            json!({"text": "and this", "images": [later]}),
            200,
        )
        .await;
    let lines = support::wait_for_codex_stdin(&anchor, 5).await;
    let turn: Value = serde_json::from_str(&lines[4]).expect("the follow-up turn/start");
    let input = turn["params"]["input"].as_array().expect("input");
    assert_eq!(input[0]["text"], "and this");
    assert_eq!(input[1], json!({"type": "localImage", "path": later}));
    probe.stop().await;
}

/// The upload fence and the count cap are the session routes': a path from
/// outside `/api/uploads` is a `409`, one that is gone another, and a body
/// over the composer's cap a `400` — all before any CLI is spawned. The API
/// backend takes text only and refuses attachments outright.
#[tokio::test]
async fn a_chat_refuses_attachments_the_session_routes_refuse() {
    let probe = support::Probe::start().await;
    let refused = |body: &Value| body["error"].as_str().unwrap_or_default().to_string();

    let (status, body) = probe
        .send("POST", "/api/chats/new", json!({"backend": "claude", "text": "hi", "images": ["/etc/passwd"]}))
        .await;
    assert_eq!(status, 409);
    assert_eq!(refused(&body), "attachment paths must come from /api/uploads");

    let uploads = probe.data_dir().join("uploads");
    std::fs::create_dir_all(&uploads).expect("uploads dir");
    let gone = uploads.join("gone.png").to_string_lossy().into_owned();
    let (status, body) = probe
        .send("POST", "/api/chats/new", json!({"backend": "codex", "text": "hi", "files": [gone]}))
        .await;
    assert_eq!(status, 409);
    assert_eq!(refused(&body), "attachment_missing");

    let image = probe.upload("shot.png", &[0x89, b'P', b'N', b'G']).await["path"]
        .as_str()
        .expect("the image path")
        .to_string();
    let over = vec![image.clone(); 21];
    let (status, body) = probe
        .send("POST", "/api/chats/new", json!({"backend": "claude", "text": "hi", "images": over}))
        .await;
    assert_eq!(status, 400);
    assert_eq!(refused(&body), "too many attachments (at most 20)");

    // The API backend answers with its own text-only call: an attachment is
    // refused, not silently dropped, and no chat is made.
    let (status, body) = probe
        .send("POST", "/api/chats/new", json!({"backend": "api", "images": [image]}))
        .await;
    assert_eq!(status, 400);
    assert_eq!(refused(&body), "attachments are not supported in api chats");
    assert_eq!(probe.get_json("/api/chats").await["chats"].as_array().expect("chats").len(), 0);

    // Nothing above started a CLI: the anchor directory is made only after the
    // fence passes.
    assert!(!probe.data_dir().join("chats").exists(), "no chat was spawned");
    probe.stop().await;
}
