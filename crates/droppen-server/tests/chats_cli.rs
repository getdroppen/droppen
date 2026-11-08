//! A CLI-backed instant chat, from the route down to the argv.
//!
//! Separate from `tests/chats.rs` because this half needs the fake CLIs on
//! `PATH` (`tests/support`) while that half needs a fake HTTP provider, and the
//! two fixtures have nothing to say to each other.
//!
//! What is asserted is the whole reason a chat is not just a session: the spawn
//! happens in the anchor directory, with the coding persona replaced and every
//! tool disabled. Attachments still arrive — a chat's images as base64 content
//! blocks and its text files inlined — without a tool that could read anything
//! else.

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
    assert_eq!(spawn.flag("--disallowedTools"), Some("*"), "a chat has no tools at all");
    assert_eq!(spawn.flag("--tools"), None, "no tool list either");
    assert_eq!(spawn.flag("--permission-mode"), None, "and no permission mode");
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

/// A claude chat has no tools, so its attachments are not paths to read: the
/// image travels as a base64 block and the text file is inlined. The prompt
/// names no image path, so the model cannot be pointed at anything a tool
/// (or a prompt injection) could open.
#[tokio::test]
async fn a_claude_chat_hands_its_attachments_to_the_cli_as_blocks() {
    let probe = support::Probe::start().await;
    let anchor = probe.data_dir().join("chats");
    let png: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];
    let image = probe.upload("shot.png", png).await["path"]
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
    assert_eq!(spawn.flag("--disallowedTools"), Some("*"), "a chat has no tools");
    assert_eq!(spawn.flag("--tools"), None, "and no tool list");
    assert_eq!(spawn.flag("--permission-mode"), None, "and no permission mode");

    let line = support::wait_for_prompt_lines(&anchor, 1).await.remove(0);
    let content = line["message"]["content"].as_array().expect("content blocks");
    assert_eq!(content.len(), 2, "the text and one image block: {line}");
    let text = content[0]["text"].as_str().expect("the text block");
    assert_eq!(
        text,
        format!("look{}", droppen_sessions::util::chat_file_block(&file, "the notes")),
        "the text file is inlined under a delimiter naming it"
    );
    assert!(!text.contains(&image), "the image is not a path in the prompt");
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["source"]["type"], "base64");
    assert_eq!(content[1]["source"]["media_type"], "image/png");
    let sent = droppen_provider::chat_attachments::image_bytes(content[1]["source"]["data"].as_str().expect("data"))
        .expect("base64");
    assert_eq!(sent, png, "the block carries the upload's own bytes");
    assert!(!line.to_string().contains(&image), "no upload path anywhere in the user line");

    // A follow-up in the same chat: a different model forces a new process, and
    // the body's permission mode must be dropped by the route.
    seed_claude_chat(&probe, &id, &anchor);
    wait_for_chat(&probe, &id).await;
    let later_png: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 4, 5, 6];
    let later = probe.upload("later.png", later_png).await["path"]
        .as_str()
        .expect("the second image path")
        .to_string();
    probe
        .send_ok(
            "POST",
            &format!("/api/sessions/claude/{id}/message"),
            json!({"text": "and this", "images": [later], "model": "other-model", "permission": "bypassPermissions"}),
            200,
        )
        .await;
    let respawned = probe.wait_for_spawn_number(&anchor, 2).await;
    assert_eq!(respawned.flag("--permission-mode"), None, "the chat's permission is pinned server-side");
    assert_eq!(respawned.flag("--disallowedTools"), Some("*"));
    assert_eq!(respawned.flag("--tools"), None);

    let lines = support::wait_for_prompt_lines(&anchor, 2).await;
    let content = lines[1]["message"]["content"].as_array().expect("content blocks");
    assert_eq!(content[0]["text"], "and this");
    assert_eq!(content[1]["type"], "image");
    let sent = droppen_provider::chat_attachments::image_bytes(content[1]["source"]["data"].as_str().expect("data"))
        .expect("base64");
    assert_eq!(sent, later_png);
    assert!(!lines[1].to_string().contains(&later));
    probe.stop().await;
}

/// A chat message with nothing attached still records a delivery row: it is
/// what stops a later identical text from reaching back to an earlier
/// message's image. The row is written after the line reaches the CLI.
#[tokio::test]
async fn a_text_only_chat_message_records_an_empty_delivery() {
    let probe = support::Probe::start().await;
    let anchor = probe.data_dir().join("chats");
    probe
        .send_ok("POST", "/api/chats/new", json!({"backend": "claude", "text": "hi"}), 200)
        .await;
    support::wait_for_prompt_lines(&anchor, 1).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let rows: (i64, String, String) = {
            let conn = rusqlite::Connection::open(probe.instance_db()).expect("instance db");
            conn.query_row(
                "SELECT count(*), coalesce(max(images), ''), coalesce(max(files), '') FROM chat_deliveries",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("deliveries")
        };
        if rows.0 > 0 {
            assert_eq!(rows.0, 1, "one message, one row");
            assert_eq!(rows.1, "[]", "no images");
            assert_eq!(rows.2, "[]", "no files");
            break;
        }
        assert!(Instant::now() < deadline, "no empty delivery row was recorded");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    probe.stop().await;
}

/// A chat send records no composer permission row: the body's mode is dropped,
/// so nothing can resurface it for a later resume.
#[tokio::test]
async fn a_chat_send_records_no_permission_row() {
    let probe = support::Probe::start().await;
    let anchor = probe.data_dir().join("chats");
    let created = probe
        .send_ok("POST", "/api/chats/new", json!({"backend": "claude", "text": "hi"}), 200)
        .await;
    let id = created["id"].as_str().expect("the chat id").to_string();
    seed_claude_chat(&probe, &id, &anchor);
    wait_for_chat(&probe, &id).await;
    probe
        .send_ok(
            "POST",
            &format!("/api/sessions/claude/{id}/message"),
            json!({"text": "next", "permission": "acceptEdits"}),
            200,
        )
        .await;
    let prefs = probe.get_json("/api/composer-prefs").await;
    assert!(
        prefs["sessions"].get(format!("claude/{id}")).is_none(),
        "a chat recorded a permission row: {prefs}"
    );
    probe.stop().await;
}

/// A message queued behind a held turn is delivered with its image when the
/// turn ends: the bytes are read at delivery, from the upload, not at send.
#[tokio::test]
async fn a_queued_chat_message_delivers_its_image_when_the_turn_ends() {
    let probe = support::Probe::start().await;
    let anchor = probe.data_dir().join("chats");
    support::plan_held_cli(&anchor);

    let created = probe
        .send_ok("POST", "/api/chats/new", json!({"backend": "claude", "text": "first"}), 200)
        .await;
    let id = created["id"].as_str().expect("the chat id").to_string();
    probe.wait_for_spawn(&anchor).await;
    support::wait_for_prompt_lines(&anchor, 1).await;
    seed_claude_chat(&probe, &id, &anchor);
    wait_for_chat(&probe, &id).await;

    let png: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 7, 8, 9];
    let image = probe.upload("queued.png", png).await["path"]
        .as_str()
        .expect("the image path")
        .to_string();
    let accepted = probe
        .send_ok(
            "POST",
            &format!("/api/sessions/claude/{id}/message"),
            json!({"text": "second", "images": [image]}),
            200,
        )
        .await;
    assert_eq!(accepted["queued"], true, "it waits behind the turn: {accepted}");

    support::release_turn(&anchor);
    let lines = support::wait_for_prompt_lines(&anchor, 2).await;
    let content = lines[1]["message"]["content"].as_array().expect("content blocks");
    assert_eq!(content[0]["text"], "second");
    assert_eq!(content[1]["type"], "image");
    let sent = droppen_provider::chat_attachments::image_bytes(content[1]["source"]["data"].as_str().expect("data"))
        .expect("base64");
    assert_eq!(sent, png);
    probe.stop().await;
}

/// Delivery re-runs the fence and the read: an upload swept while its message
/// waited behind a turn fails the turn instead of being written as an empty
/// block or read from wherever the path now points.
#[tokio::test]
async fn a_queued_message_whose_upload_was_swept_is_not_sent() {
    let probe = support::Probe::start().await;
    let anchor = probe.data_dir().join("chats");
    support::plan_held_cli(&anchor);
    let created = probe
        .send_ok("POST", "/api/chats/new", json!({"backend": "claude", "text": "first"}), 200)
        .await;
    let id = created["id"].as_str().expect("the chat id").to_string();
    probe.wait_for_spawn(&anchor).await;
    support::wait_for_prompt_lines(&anchor, 1).await;
    seed_claude_chat(&probe, &id, &anchor);
    wait_for_chat(&probe, &id).await;

    let uploaded = probe.upload("swept.png", &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1]).await;
    let path = uploaded["path"].as_str().expect("the image path").to_string();
    probe
        .send_ok(
            "POST",
            &format!("/api/sessions/claude/{id}/message"),
            json!({"text": "second", "images": [path]}),
            200,
        )
        .await;
    // The queue holds the paths, not the bytes; the upload goes away first.
    std::fs::remove_file(&path).expect("sweep the upload");
    support::release_turn(&anchor);

    // Wait for the held turn to end and the queued one to be refused.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let detail = probe.get_json(&format!("/api/sessions/claude/{id}")).await;
        if detail["summary"]["resident"]["phase"] == "idle" && support::prompt_lines(&anchor).len() == 1 {
            break;
        }
        assert!(Instant::now() < deadline, "the swept message never settled: {detail}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        support::prompt_lines(&anchor).len(),
        1,
        "a swept upload must not be written to the CLI"
    );
    probe.stop().await;
}

/// Delivery follows no link: an upload swapped for a symlink to a file outside
/// uploads after the send is refused, not read.
#[cfg(unix)]
#[tokio::test]
async fn a_queued_message_whose_upload_became_a_symlink_is_not_sent() {
    let probe = support::Probe::start().await;
    let anchor = probe.data_dir().join("chats");
    support::plan_held_cli(&anchor);
    let created = probe
        .send_ok("POST", "/api/chats/new", json!({"backend": "claude", "text": "first"}), 200)
        .await;
    let id = created["id"].as_str().expect("the chat id").to_string();
    probe.wait_for_spawn(&anchor).await;
    support::wait_for_prompt_lines(&anchor, 1).await;
    seed_claude_chat(&probe, &id, &anchor);
    wait_for_chat(&probe, &id).await;

    let uploaded = probe.upload("linked.txt", b"the real upload").await;
    let path = uploaded["path"].as_str().expect("the file path").to_string();
    probe
        .send_ok(
            "POST",
            &format!("/api/sessions/claude/{id}/message"),
            json!({"text": "second", "files": [path]}),
            200,
        )
        .await;
    let outside = probe.home.join("outside.txt");
    std::fs::write(&outside, b"outside the uploads directory").expect("outside file");
    std::fs::remove_file(&path).expect("remove the upload");
    std::os::unix::fs::symlink(&outside, &path).expect("swap in a symlink");
    support::release_turn(&anchor);

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let detail = probe.get_json(&format!("/api/sessions/claude/{id}")).await;
        if detail["summary"]["resident"]["phase"] == "idle" && support::prompt_lines(&anchor).len() == 1 {
            break;
        }
        assert!(Instant::now() < deadline, "the linked message never settled: {detail}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        support::prompt_lines(&anchor).len(),
        1,
        "a symlinked upload must not be read through"
    );
    probe.stop().await;
}

/// A session is unchanged: its image is still a path in the prompt for the
/// Read tool, and its process still has the ordinary tool set.
#[tokio::test]
async fn a_session_keeps_its_path_based_attachment_delivery() {
    let probe = support::Probe::start().await;
    let workspace = probe.workspace();
    let image = probe.upload("shot.png", &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]).await["path"]
        .as_str()
        .expect("the image path")
        .to_string();
    probe
        .send_ok(
            "POST",
            "/api/sessions/new",
            json!({"source": "claude", "cwd": workspace.to_string_lossy(), "text": "look", "images": [image]}),
            200,
        )
        .await;
    let spawn = probe.wait_for_spawn(&workspace).await;
    assert_eq!(spawn.flag("--disallowedTools"), Some("EnterPlanMode"), "the ordinary deny list");
    assert_eq!(spawn.flag("--tools"), None);
    let line = support::wait_for_prompt_lines(&workspace, 1).await.remove(0);
    let content = line["message"]["content"].as_array().expect("content blocks");
    assert_eq!(content.len(), 1, "a session's image is a path, not a block");
    assert_eq!(
        content[0]["text"],
        format!("look\n\n[1 attached file(s) — view with the Read tool:]\n- {image}")
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

    // A claude chat inlines text files and base64-encodes images: a binary
    // file and an image over the API's 5 MB are both refused here, clearly.
    let binary = probe.upload("binary.bin", &[0xff, 0xfe, 0x00]).await["path"]
        .as_str()
        .expect("the binary path")
        .to_string();
    let (status, body) = probe
        .send("POST", "/api/chats/new", json!({"backend": "claude", "text": "hi", "files": [binary]}))
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(refused(&body), "binary.bin is not a UTF-8 text file a claude chat can take");

    let oversized: Vec<u8> = [&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a][..], &vec![0u8; 5 * 1024 * 1024]].concat();
    let big = probe.upload("huge.png", &oversized).await["path"]
        .as_str()
        .expect("the big image path")
        .to_string();
    let (status, body) = probe
        .send("POST", "/api/chats/new", json!({"backend": "claude", "text": "hi", "images": [big]}))
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        refused(&body),
        "huge.png is too large for a claude chat (images are at most 5 MB)"
    );

    // Text files together over the message cap: three 80 KB files, each under
    // the per-file cap, exceed the 200 KB total.
    let mut texts = Vec::new();
    for index in 0..3 {
        let path = probe.upload(&format!("notes-{index}.txt"), &vec![b'a'; 80 * 1024]).await["path"]
            .as_str()
            .expect("the text path")
            .to_string();
        texts.push(path);
    }
    let (status, body) = probe
        .send("POST", "/api/chats/new", json!({"backend": "claude", "text": "hi", "files": texts}))
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        refused(&body),
        "the attached text files are too large together for a claude chat (at most 200 KB)"
    );

    // Images together over the message cap: five 4.1 MB images, each under the
    // per-image cap, exceed the 20 MB total (and the base64 line's limit).
    let mut images = Vec::new();
    for index in 0..5 {
        let mut bytes = vec![0u8; 4 * 1024 * 1024 + 100 * 1024];
        bytes[..8].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        let path = probe.upload(&format!("big-{index}.png"), &bytes).await["path"]
            .as_str()
            .expect("the image path")
            .to_string();
        images.push(path);
    }
    let (status, body) = probe
        .send("POST", "/api/chats/new", json!({"backend": "claude", "text": "hi", "images": images}))
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        refused(&body),
        "the attached images are too large together for a claude chat (at most 20 MB)"
    );

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
