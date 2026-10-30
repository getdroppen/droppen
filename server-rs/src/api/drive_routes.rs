//! The drive's routes (`droppen_drive::routes`) on the drive the server
//! builds, with its providers, logins and transcript readers (`crate::app`):
//! where a session may run, what a chat may attach, and search in the feed's
//! numbering.

use std::sync::Arc;

use gotcha::State;

use droppen_auth::MaybeIdentity;
use droppen_http::extract::{ApiJson, ApiPath};
use droppen_http::openapi_ext::Optional;
use droppen_sessions::{chats_cwd, file_signature, SessionScope, Source};

use droppen_drive::routes::{send_message, SearchQuery, SendMessage};
use droppen_drive::{Drive, DriveState};
use droppen_drive::feed;
use crate::provider::claude_code::sessions::claude;

fn drive() -> (tempfile::TempDir, Arc<Drive>) {
    drive_with(crate::app::register_providers)
}

/// [`drive`], with the providers `register` adds.
fn drive_with(register: fn(&mut droppen_provider::ProviderRegistry, &Arc<droppen_provider::InjectionRegistry>)) -> (tempfile::TempDir, Arc<Drive>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Arc::new(droppen_instance::paths::Paths::new(dir.path().to_path_buf()));
    let instance = droppen_instance::instance::InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE).expect("valid name");
    let auth = droppen_auth::Auth::new(&paths, &instance, &droppen_http::config::AuthSettings::default(), false, std::sync::Arc::new(crate::app::NotifyTokens));
    let drive = Drive::new(paths.clone(), &instance, auth, crate::app::transcript_sources(), register, crate::app::logins(), crate::app::NotifyDecoration::new(paths.clone(), instance.clone()));
    (dir, drive)
}

/// The route's own contract, through the shared helper it now delegates to.
#[test]
fn a_cwd_can_never_leave_the_home_directory() {
    let (dir, drive) = drive();
    let home = dir.path();
    let resolve = |requested| droppen_files::resolve_safe(&drive.paths, Some(requested));
    assert_eq!(resolve("~").expect("home"), home);
    assert_eq!(resolve("~/proj").expect("under home"), home.join("proj"));
    assert_eq!(resolve("").expect("empty is home"), home);
    assert!(resolve("/etc").is_err());
    assert!(resolve("~/../..").is_err(), "a traversal is caught after normalisation");
}

/// An upload on disk, as `POST /api/uploads` leaves one: `uploads/<dir>/<name>`.
fn uploaded(drive: &Drive, name: &str) -> String {
    let path = drive.paths.uploads_dir().join("abc").join(name);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("uploads dir");
    std::fs::write(&path, b"x").expect("upload");
    path.to_string_lossy().into_owned()
}

/// A message with an attachment to a Claude Code instant chat is refused:
/// the chat runs with every tool off. So is one to a chat no provider runs,
/// which the send block turns away before the chat check is reached.
#[tokio::test]
async fn a_chat_that_cannot_read_an_attachment_refuses_one() {
    let cases: [(fn(&mut droppen_provider::ProviderRegistry, &Arc<droppen_provider::InjectionRegistry>), &str); 2] = [
        (crate::app::register_providers, "attachments are not supported in claude instant chats"),
        (|_, _| {}, "no provider runs claude sessions"),
    ];
    for (register, expected) in cases {
        let (_dir, drive) = drive_with(register);
        let id = "11111111-2222-4333-8444-555555555555";
        let project = crate::provider::claude_code::claude_roots::ClaudeRoots::of(&drive.paths).managed_projects().join("-chats");
        std::fs::create_dir_all(&project).expect("project");
        let chats = chats_cwd(&drive.paths).to_string_lossy().into_owned();
        let line = serde_json::json!({
            "type": "user", "sessionId": id, "cwd": chats, "uuid": "u1", "timestamp": "2026-10-01T10:00:00Z",
            "message": {"role": "user", "content": "hi"},
        });
        std::fs::write(project.join(format!("{id}.jsonl")), format!("{line}\n")).expect("transcript");
        let state = DriveState {
            drive: drive.clone(),
            paths: drive.paths.clone(),
            global: droppen_drive::feed::global::GlobalFeed::new(),
            refresh: droppen_http::refresh::Refresh::new(droppen_drive::feed::global::GlobalFeed::new()),
        };
        let body = SendMessage {
            text: "look".into(),
            images: Optional(Some(vec![uploaded(&drive, "a.png")])),
            files: Optional(None),
            model: Optional(None),
            effort: Optional(None),
            permission: Optional(None),
            account_id: None,
            client_message_id: Optional(None),
        };
        let error = send_message(State(state), MaybeIdentity(None), ApiPath((Source::Claude, id.to_string())), ApiJson(body))
            .await
            .expect_err("refused");
        assert_eq!((error.status().as_u16(), error.body()["error"].as_str()), (409, Some(expected)));
    }
}

fn search(q: &str) -> SearchQuery {
    SearchQuery {
        q: q.to_string(),
        limit: None,
    }
}

/// A claude transcript in the user's own `~/.claude`, which is a session
/// this server did not start.
fn external_claude_session(drive: &Drive, id: &str, lines: &[serde_json::Value]) {
    let project = crate::provider::claude_code::claude_roots::ClaudeRoots::of(&drive.paths).local_projects().join("-w-repo");
    std::fs::create_dir_all(&project).expect("project");
    let body: String = lines.iter().map(|line| format!("{line}\n")).collect();
    std::fs::write(project.join(format!("{id}.jsonl")), body).expect("transcript");
}

/// The first unread is found against the marker the client kept, in the
/// numbering the history route pages by — and like search, for a session
/// that is not this server's own.
#[tokio::test]
async fn the_first_unread_is_an_index_in_the_feeds_numbering() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, drive) = drive();
    let id = "88888888-8888-4888-8888-888888888888";
    let entry = |kind: &str, uuid: &str, at: &str, content: serde_json::Value| serde_json::json!({"type":kind,"message":{"role":kind,"content":content},"timestamp":at,"cwd":"/w/repo","sessionId":id,"uuid":uuid});
    external_claude_session(
        &drive,
        id,
        &[
            entry("user", "u1", "2026-10-01T10:00:00.000Z", serde_json::json!("hi")),
            entry(
                "assistant",
                "a1",
                "2026-10-01T10:00:01.000Z",
                serde_json::json!([{"type":"text","text":"first answer"}]),
            ),
            entry("user", "u2", "2026-10-01T10:01:00.000Z", serde_json::json!("more")),
            entry(
                "assistant",
                "t1",
                "2026-10-01T10:01:01.000Z",
                serde_json::json!([{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]),
            ),
            entry(
                "user",
                "r1",
                "2026-10-01T10:01:02.000Z",
                serde_json::json!([{"type":"tool_result","tool_use_id":"toolu_1","content":"a b"}]),
            ),
            entry(
                "assistant",
                "a2",
                "2026-10-01T10:01:03.000Z",
                serde_json::json!([{"type":"text","text":"second answer"}]),
            ),
            entry(
                "assistant",
                "a3",
                "2026-10-01T10:01:04.000Z",
                serde_json::json!([{"type":"text","text":"third answer"}]),
            ),
        ],
    );
    let summary = claude::load_claude_summary(&drive.paths, id).await.expect("the session is there");
    let sig = file_signature(std::path::Path::new(&summary.file_path)).await.expect("sig");
    let items = drive
        .transcripts
        .load_items(&drive.paths, Source::Claude, std::path::Path::new(&summary.file_path), &sig)
        .await;

    let found = drive
        .first_unread_of(Source::Claude, id, Some("2026-10-01T10:00:30.000Z"))
        .await
        .expect("readable");
    assert_eq!(found.count, 2);
    let index = found.index.0.expect("an unread message");
    assert_eq!(items[index].text, "second answer", "index {index} is the message it names");

    let never = drive.first_unread_of(Source::Claude, id, None).await.expect("readable");
    assert_eq!(
        (never.index.0.map(|index| items[index].text.as_str()), never.count),
        (Some("first answer"), 3)
    );
    let read = drive
        .first_unread_of(Source::Claude, id, Some("2026-10-01T11:00:00.000Z"))
        .await
        .expect("readable");
    assert_eq!((read.index.0, read.count), (None, 0), "nothing newer than the marker");
    assert!(drive
        .first_unread_of(Source::Claude, "77777777-7777-4777-8777-777777777777", None)
        .await
        .is_err());
}

/// Reading is never gated: somebody else's session is searched exactly like
/// one of this server's own, in the feed's numbering.
#[tokio::test]
async fn an_external_session_is_searchable_in_the_feeds_numbering() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, drive) = drive();
    let id = "55555555-5555-4555-8555-555555555555";
    let entry = |kind: &str, uuid: &str, content: serde_json::Value| serde_json::json!({"type":kind,"message":{"role":kind,"content":content},"timestamp":"2026-10-01T10:00:00.000Z","cwd":"/w/repo","sessionId":id,"uuid":uuid});
    external_claude_session(
        &drive,
        id,
        &[
            entry("user", "u1", serde_json::json!("why does the Login page flicker?")),
            entry(
                "assistant",
                "a1",
                serde_json::json!([{"type":"text","text":"The login page re-renders on every tick."}]),
            ),
            entry(
                "assistant",
                "t1",
                serde_json::json!([{"type":"tool_use","id":"toolu_1","name":"Grep","input":{"pattern":"useTicker","path":"src/login"}}]),
            ),
            entry(
                "user",
                "r1",
                serde_json::json!([{"type":"tool_result","tool_use_id":"toolu_1","content":"src/login/page.tsx:12: useTicker()"}]),
            ),
        ],
    );
    let summary = claude::load_claude_summary(&drive.paths, id).await.expect("the session is there");
    assert_eq!(summary.scope, SessionScope::External, "not this server's own");

    let found = drive.search_session(Source::Claude, id, &search("LOGIN page")).await.expect("searchable");
    assert_eq!(found.total, 3);
    let indices: Vec<usize> = found.matches.iter().map(|m| m.index).collect();
    assert_eq!(indices, vec![0, 1, 3], "the result's `src/login/page.tsx` holds both terms too");
    assert_eq!(found.query, "LOGIN page");

    // The same numbering the history route pages by.
    let sig = file_signature(std::path::Path::new(&summary.file_path)).await.expect("sig");
    let items = drive
        .transcripts
        .load_items(&drive.paths, Source::Claude, std::path::Path::new(&summary.file_path), &sig)
        .await;
    let ticker = drive.search_session(Source::Claude, id, &search("useticker")).await.expect("searchable");
    for hit in &ticker.matches {
        let item = &items[hit.index];
        assert_eq!(
            (hit.role, hit.kind),
            (item.role, item.kind),
            "index {} is the item it describes",
            hit.index
        );
    }
    let fields: Vec<feed::search::SearchField> = ticker.matches.iter().map(|m| m.field).collect();
    assert_eq!(
        fields,
        vec![feed::search::SearchField::Detail, feed::search::SearchField::Detail],
        "the pattern is in the call's input and in the result's output"
    );
}

#[tokio::test]
async fn a_codex_thread_is_searchable_too() {
    let _guard = droppen_sessions::index::exclusive().await;
    let (_dir, drive) = drive();
    let id = "66666666-6666-4666-8666-666666666666";
    let day = drive.paths.codex_sessions().join("2026").join("10").join("01");
    std::fs::create_dir_all(&day).expect("rollout dir");
    let lines = [
        r#"{"type":"session_meta","payload":{"cwd":"/w","id":"x"}}"#,
        r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"列出 迁移脚本"}]}}"#,
        r#"{"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"look for the migrations first"}]}}"#,
        r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"ls migrations\"}"}}"#,
        r#"{"type":"response_item","payload":{"type":"function_call_output","output":"0001_init.sql\n0002_迁移.sql"}}"#,
    ];
    std::fs::write(day.join(format!("rollout-2026-10-01T10-00-00-{id}.jsonl")), lines.join("\n") + "\n").expect("rollout");

    let found = drive.search_session(Source::Codex, id, &search("迁移")).await.expect("searchable");
    let shape: Vec<(usize, droppen_sessions::util::ItemKind)> = found.matches.iter().map(|m| (m.index, m.kind)).collect();
    assert_eq!(found.total, 2, "{shape:?}");
    assert_eq!(found.matches[0].field, feed::search::SearchField::Text, "the user's own words");
    assert_eq!(found.matches[1].field, feed::search::SearchField::Detail, "the command's output");

    let calls = drive
        .search_session(Source::Codex, id, &search("ls MIGRATIONS"))
        .await
        .expect("searchable");
    assert_eq!(calls.total, 1);
    assert_eq!(calls.matches[0].field, feed::search::SearchField::Tool);
    assert_eq!(calls.matches[0].snippet, "shell · ls migrations");
}

#[tokio::test]
async fn an_empty_query_is_refused_before_the_session_is_looked_up() {
    let (_dir, drive) = drive();
    for blank in ["", "   ", "\t\n"] {
        let error = drive
            .search_session(Source::Claude, "no-such-session", &search(blank))
            .await
            .expect_err("refused");
        assert_eq!(error.status(), gotcha::axum::http::StatusCode::BAD_REQUEST, "{blank:?}");
    }
    let error = drive
        .search_session(Source::Claude, "no-such-session", &search("x"))
        .await
        .expect_err("missing");
    assert_eq!(error.status(), gotcha::axum::http::StatusCode::NOT_FOUND);
}

/// A message to a session this server did not start is refused with its CLI's
/// own reason: Codex's, for a thread in the user's own `~/.codex`.
#[tokio::test]
async fn a_message_to_a_codex_thread_started_elsewhere_is_refused_with_codex_s_reason() {
    use gotcha::axum::extract::FromRef;
    let _guard = droppen_sessions::index::exclusive().await;
    let home = tempfile::tempdir().expect("tempdir");
    let state = crate::api::test_support::state(home.path()).await;
    let id = "77777777-7777-4777-8777-777777777777";
    let day = state.drive.paths.codex_sessions().join("2026").join("10").join("01");
    std::fs::create_dir_all(&day).expect("rollout dir");
    let meta = r#"{"type":"session_meta","payload":{"cwd":"/w","id":"x"}}"#;
    std::fs::write(day.join(format!("rollout-2026-10-01T10-00-00-{id}.jsonl")), format!("{meta}\n")).expect("rollout");
    let body = SendMessage {
        text: "hi".into(),
        images: Optional(None),
        files: Optional(None),
        model: Optional(None),
        effort: Optional(None),
        permission: Optional(None),
        account_id: None,
        client_message_id: Optional(None),
    };
    let error = send_message(State(DriveState::from_ref(&state)), MaybeIdentity(None), ApiPath((Source::Codex, id.to_string())), ApiJson(body))
        .await
        .expect_err("refused");
    assert_eq!(error.status().as_u16(), 409);
    assert_eq!(
        error.body().to_string(),
        r#"{"error":"external_session","hint":"this codex session was started elsewhere; continue it where it was started"}"#
    );
}
