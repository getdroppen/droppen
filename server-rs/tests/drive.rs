//! The rest of the session-driving surface: rename, archive, older transcript
//! pages, subagent transcripts and the composer preferences.

mod support;

use std::io::Write;

use remote_hand_server::db::scope;
use serde_json::{json, Value};
use support::{common, Probe};

const SESSION_ID: &str = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";

#[tokio::test]
async fn pins_are_persisted_listed_and_validated_without_changing_transcripts() {
    let probe = Probe::start().await;
    let path = probe.seed_session(&probe.managed_root(), SESSION_ID, &probe.workspace());
    let before = std::fs::read(&path).unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
        .unwrap();
    let newer = "cccccccc-3333-4333-8333-cccccccccccc";
    probe.seed_session(&probe.managed_root(), newer, &probe.workspace());
    let route = format!("/api/sessions/claude/{SESSION_ID}/pin");
    assert_eq!(probe.get_json("/api/sessions?limit=1").await["sessions"][0]["id"], newer);
    assert_eq!(probe.get_json("/api/sessions").await["sessions"][0]["pinned"], false);
    for _ in 0..2 {
        assert_eq!(probe.send_ok("PUT", &route, json!({"pinned": true}), 200).await["pinned"], true);
    }
    assert_eq!(probe.get_json("/api/sessions?limit=1").await["sessions"][0]["pinned"], true);
    assert_eq!(
        probe.get_json("/api/sessions?limit=1").await["sessions"][0]["id"],
        SESSION_ID,
        "pins are ranked before pagination"
    );
    assert_eq!(std::fs::read(path).unwrap(), before);
    probe.send_ok("PUT", &route, json!({"pinned": false}), 200).await;
    assert_eq!(probe.get_json("/api/sessions").await["sessions"][0]["pinned"], false);
    for body in [json!({}), json!({"pinned": "true"}), json!({"pinned": null})] {
        assert_eq!(probe.send("PUT", &route, body).await.0, 400);
    }
    assert_eq!(
        probe.send("PUT", "/api/sessions/claude/missing/pin", json!({"pinned": true})).await.0,
        404
    );
    let external = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb";
    probe.seed_session(&probe.local_root(), external, &probe.workspace());
    assert_eq!(
        probe
            .send("PUT", &format!("/api/sessions/claude/{external}/pin"), json!({"pinned": true}))
            .await
            .0,
        409
    );
}

#[tokio::test]
async fn renaming_outranks_the_derived_title_and_an_empty_name_resets_it() {
    let probe = Probe::start().await;
    // Seeded into the root this server owns, so the plain list shows it: the
    // default list is this server's own sessions.
    probe.seed_session(&probe.managed_root(), SESSION_ID, &probe.workspace());

    let renamed = probe
        .send_ok(
            "PUT",
            &format!("/api/sessions/claude/{SESSION_ID}/title"),
            json!({"title": "  Ported  "}),
            200,
        )
        .await;
    assert_eq!(renamed["title"], "Ported", "the stored title is trimmed");
    assert_eq!(probe.get_json("/api/sessions").await["sessions"][0]["title"], "Ported");

    let reset = probe
        .send_ok("PUT", &format!("/api/sessions/claude/{SESSION_ID}/title"), json!({"title": "   "}), 200)
        .await;
    assert_eq!(reset["title"], "");
    assert_eq!(
        probe.get_json("/api/sessions").await["sessions"][0]["title"],
        "hello",
        "back to the title derived from the transcript"
    );

    let (status, body) = probe
        .send("PUT", "/api/sessions/claude/does-not-exist/title", json!({"title": "x"}))
        .await;
    assert_eq!(status, 404);
    assert_eq!(body["error"], "session not found");

    probe.stop().await;
}

#[tokio::test]
async fn archiving_hides_a_claude_session_from_every_lookup() {
    let probe = Probe::start().await;
    probe.seed_session(&probe.managed_root(), SESSION_ID, &probe.workspace());
    assert_eq!(probe.get_json("/api/sessions").await["total"], 1);

    let archived = probe
        .send_ok("POST", &format!("/api/sessions/claude/{SESSION_ID}/archive"), json!({}), 200)
        .await;
    assert_eq!(archived["accepted"], true);

    assert_eq!(
        probe.get_json("/api/sessions").await["total"],
        0,
        "the hidden list is honoured by the list"
    );
    assert_eq!(
        probe.get(&format!("/api/sessions/claude/{SESSION_ID}")).await.0,
        404,
        "…and by a direct lookup"
    );
    // Archiving it again cannot find it any more, which is a 404 not a crash.
    let (status, _) = probe.send("POST", &format!("/api/sessions/claude/{SESSION_ID}/archive"), json!({})).await;
    assert_eq!(status, 404);

    // One row per hidden session, in the *machine-wide* store rather than this
    // instance's, and that is the half the assertions above cannot see: hiding a
    // session hides it from every server on this machine, which is what the user
    // means by archiving it. The value is unused — the row is the fact.
    assert_eq!(
        common::rows(&probe.config_db(), "SELECT key FROM kv WHERE scope = ?1", (scope::ARCHIVED,)),
        [SESSION_ID]
    );

    probe.stop().await;
}

#[tokio::test]
async fn an_older_page_of_the_transcript_can_be_paged_back_to() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    let path = probe.seed_session(&probe.local_root(), SESSION_ID, &workspace);

    let mut file = std::fs::OpenOptions::new().append(true).open(&path).expect("append");
    for index in 0..250 {
        writeln!(
            file,
            "{}",
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": format!("line {index}")}]}})
        )
        .expect("write");
    }
    drop(file);

    let detail = probe.get_json(&format!("/api/sessions/claude/{SESSION_ID}")).await;
    assert_eq!(detail["totalItems"], 251);
    assert_eq!(detail["startIndex"], 51, "a page is the last two hundred items");
    assert_eq!(detail["hasMoreBefore"], true);

    let page = probe.get_json(&format!("/api/sessions/claude/{SESSION_ID}/history?before=51")).await;
    assert_eq!(page["startIndex"], 0);
    assert_eq!(page["endIndex"], 51);
    assert_eq!(page["totalItems"], 251);
    assert_eq!(page["hasMoreBefore"], false);
    assert_eq!(page["items"].as_array().expect("items").len(), 51);
    assert_eq!(page["items"][0]["text"], "hello", "the page ends where the detail page began");
    assert_eq!(page["sig"], detail["sig"]);

    let (status, body) = probe.get("/api/sessions/claude/does-not-exist/history?before=0").await;
    assert_eq!(status, 404, "{body}");

    probe.stop().await;
}

#[tokio::test]
async fn a_subagent_transcript_is_readable_through_its_task_call() {
    let probe = Probe::start().await;
    let path = probe.seed_session(&probe.local_root(), SESSION_ID, &probe.workspace());

    // The Task call that spawned it, plus the subagent's own files beside it.
    let mut file = std::fs::OpenOptions::new().append(true).open(&path).expect("append");
    writeln!(
        file,
        "{}",
        json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Task", "input": {"description": "explore"}}]},
        })
    )
    .expect("write");
    drop(file);

    let subagents = path.with_extension("").join("subagents");
    std::fs::create_dir_all(&subagents).expect("subagents dir");
    std::fs::write(
        subagents.join("agent-abc.meta.json"),
        json!({"toolUseId": "t1", "description": "explore the repo", "agentType": "Explore"}).to_string(),
    )
    .expect("meta");
    std::fs::write(
        subagents.join("agent-abc.jsonl"),
        format!(
            "{}\n",
            json!({
                "type": "assistant",
                "isSidechain": true,
                "message": {"content": [{"type": "text", "text": "found it"}]},
            })
        ),
    )
    .expect("subagent transcript");

    let detail = probe.get_json(&format!("/api/sessions/claude/{SESSION_ID}")).await;
    let task = detail["items"].as_array().expect("items").iter().find(|item| item["toolName"] == "Task");
    assert_eq!(
        task.expect("the Task call")["agent"],
        "abc",
        "the call is tagged with the agent it spawned"
    );

    let agent = probe.get_json(&format!("/api/sessions/claude/{SESSION_ID}/agents/abc")).await;
    assert_eq!(agent["agentType"], "Explore");
    assert_eq!(agent["description"], "explore the repo");
    assert_eq!(agent["totalItems"], 1);
    assert_eq!(agent["omittedItems"], 0);
    assert_eq!(
        agent["items"][0]["text"], "found it",
        "sidechain entries are kept for the subagent's own view"
    );

    for (path, reason) in [
        (
            format!("/api/sessions/claude/{SESSION_ID}/agents/nope"),
            "an agent that was never spawned",
        ),
        (format!("/api/sessions/codex/{SESSION_ID}/agents/abc"), "subagents exist only for claude"),
    ] {
        assert_eq!(probe.get(&path).await.0, 404, "{reason}");
    }
    assert_eq!(
        probe.get(&format!("/api/sessions/claude/{SESSION_ID}/agents/..%2Fescape")).await.0,
        400,
        "a traversing id is malformed, which the TypeScript rejects before the handler"
    );

    probe.stop().await;
}

/// The fallback index for work the transcript cannot point at.
///
/// Two shapes have no `Task` call to hang off in the session's own file: a
/// Skill-spawned agent, whose meta carries no `toolUseId`, and anything at
/// `spawnDepth` 2 or deeper, whose `toolUseId` names a call in its *parent
/// agent's* transcript. One real session hid eleven agents and 2.5 MB of work
/// that way, so this route lists everything on disk, anchored or not.
#[tokio::test]
async fn every_subagent_is_listed_even_when_the_transcript_cannot_reach_it() {
    let probe = Probe::start().await;
    let path = probe.seed_session(&probe.local_root(), SESSION_ID, &probe.workspace());

    let mut file = std::fs::OpenOptions::new().append(true).open(&path).expect("append");
    writeln!(
        file,
        "{}",
        json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Task", "input": {"description": "explore"}}]},
        })
    )
    .expect("write");
    drop(file);

    let subagents = path.with_extension("").join("subagents");
    std::fs::create_dir_all(subagents.join("workflows")).expect("subagents dir");
    let agent = |dir: &std::path::Path, id: &str, meta: Value| {
        if !meta.is_null() {
            std::fs::write(dir.join(format!("agent-{id}.meta.json")), meta.to_string()).expect("meta");
        }
        std::fs::write(
            dir.join(format!("agent-{id}.jsonl")),
            format!(
                "{}\n",
                json!({"type": "assistant", "isSidechain": true, "message": {"content": [{"type": "text", "text": id}]}})
            ),
        )
        .expect("agent transcript");
    };
    agent(&subagents, "abc", json!({"toolUseId": "t1", "agentType": "Explore", "spawnDepth": 1}));
    agent(&subagents, "skill", json!({"agentType": "general-purpose", "spawnDepth": 1}));
    agent(
        &subagents,
        "deep",
        json!({"toolUseId": "toolu_inside_skill", "parentAgentId": "skill", "spawnDepth": 2}),
    );
    agent(&subagents.join("workflows"), "flow", Value::Null);

    let listed = probe.get_json(&format!("/api/sessions/claude/{SESSION_ID}/agents")).await;
    let agents = listed["agents"].as_array().expect("agents").clone();
    let by_id = |id: &str| {
        agents
            .iter()
            .find(|agent| agent["agentId"] == id)
            .unwrap_or_else(|| panic!("{id} is missing from {agents:?}"))
            .clone()
    };
    assert_eq!(agents.len(), 4, "including the ones nothing can anchor: {agents:?}");
    assert_eq!(by_id("abc")["anchored"], true);
    assert_eq!(by_id("abc")["agentType"], "Explore");
    assert_eq!(by_id("skill")["anchored"], false, "a Skill's agent has no toolUseId at all");
    assert_eq!(
        by_id("deep")["anchored"],
        false,
        "a grandchild's toolUseId names a call in another transcript"
    );
    assert_eq!(by_id("deep")["spawnDepth"], 2);
    assert_eq!(by_id("deep")["parentAgentId"], "skill");
    assert!(by_id("flow")["bytes"].as_u64().expect("bytes") > 0);
    assert!(by_id("flow")["updatedAt"].is_string());

    // Listed means readable: the one filed under a subdirectory too.
    let nested = probe.get_json(&format!("/api/sessions/claude/{SESSION_ID}/agents/flow")).await;
    assert_eq!(nested["items"][0]["text"], "flow");

    assert_eq!(
        probe.get(&format!("/api/sessions/codex/{SESSION_ID}/agents")).await.0,
        404,
        "subagents exist only for claude sessions"
    );
    probe.stop().await;
}

#[tokio::test]
async fn assistant_text_grows_chips_for_files_that_really_exist() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    let path = probe.seed_session(&probe.local_root(), SESSION_ID, &workspace);
    std::fs::write(workspace.join("notes.md"), "hi").expect("real file");

    let mut file = std::fs::OpenOptions::new().append(true).open(&path).expect("append");
    writeln!(
        file,
        "{}",
        json!({
            "type": "assistant",
            "message": {"content": [{"type": "text", "text": "wrote `notes.md` and `ghost.md`"}]},
        })
    )
    .expect("write");
    drop(file);

    let detail = probe.get_json(&format!("/api/sessions/claude/{SESSION_ID}")).await;
    let last = detail["items"].as_array().expect("items").last().expect("assistant item").clone();
    let files = last["files"].as_array().expect("chips");
    assert_eq!(files.len(), 1, "only the file that exists is offered: {last}");
    assert_eq!(files[0]["name"], "notes.md");
    assert_eq!(files[0]["path"], workspace.join("notes.md").to_string_lossy().to_string());

    probe.stop().await;
}

#[tokio::test]
async fn a_composer_choice_becomes_both_a_session_entry_and_a_source_default() {
    let probe = Probe::start().await;

    let empty = probe.get_json("/api/composer-prefs").await;
    assert_eq!(empty["defaults"], json!({}));
    assert_eq!(empty["sessions"], json!({}));

    let updated = probe
        .send_ok("PUT", "/api/composer-prefs/claude/abc", json!({"model": "opus", "effort": "high"}), 200)
        .await;
    assert_eq!(updated["defaults"]["claude"]["model"], "opus");
    assert_eq!(updated["sessions"]["claude/abc"]["effort"], "high");
    assert!(updated["sessions"]["claude/abc"]["updatedAt"].as_str().is_some());

    // A source default alone leaves the session bucket untouched.
    let defaults_only = probe
        .send_ok("PUT", "/api/composer-prefs/codex", json!({"permission": "read-only"}), 200)
        .await;
    assert_eq!(defaults_only["defaults"]["codex"]["permission"], "read-only");
    assert_eq!(defaults_only["sessions"].as_object().expect("sessions").len(), 1);

    // A partial patch merges rather than replacing.
    let merged = probe
        .send_ok("PUT", "/api/composer-prefs/claude/abc", json!({"effort": "low"}), 200)
        .await;
    assert_eq!(merged["sessions"]["claude/abc"]["model"], "opus", "the untouched field survives");
    assert_eq!(merged["sessions"]["claude/abc"]["effort"], "low");

    // Values that could read as CLI flags never reach the file.
    let (status, _) = probe.send("PUT", "/api/composer-prefs/claude", json!({"model": "--add-dir"})).await;
    assert_eq!(status, 400);

    let reread = probe.get_json("/api/composer-prefs").await;
    assert_eq!(reread["defaults"]["claude"]["model"], "opus", "and the refused patch changed nothing");

    probe.stop().await;
}

#[tokio::test]
async fn a_malformed_request_is_a_400_with_a_json_error() {
    let probe = Probe::start().await;
    probe.seed_session(&probe.local_root(), SESSION_ID, &probe.workspace());

    // axum answers a well-shaped-but-wrong body with a `422 text/plain`, a
    // missing content type with a `415`, and an unparseable path segment with a
    // text `400`. `@hono/zod-validator` answers all three with a JSON 400, and
    // the web client reads `.error` off the response.
    for (method, path, body) in [
        ("POST", format!("/api/sessions/claude/{SESSION_ID}/message"), json!({})),
        ("POST", format!("/api/sessions/claude/{SESSION_ID}/message"), json!({"text": 7})),
        ("POST", "/api/accounts".to_string(), json!({"label": "x", "kind": "bogus", "secret": "y"})),
        ("PUT", "/api/accounts/default".to_string(), json!({})),
        ("PUT", "/api/composer-prefs/nonsense".to_string(), json!({})),
    ] {
        let (status, value) = probe.send(method, &path, body).await;
        assert_eq!(status, 400, "{method} {path} -> {status}: {value}");
        assert!(value["error"].as_str().is_some(), "a failed request always carries `error`: {value}");
    }

    // A query parameter the route requires is the same story.
    let (status, raw) = probe.get(&format!("/api/sessions/claude/{SESSION_ID}/history")).await;
    assert_eq!(status, 400);
    assert!(raw.contains("\"error\""), "{raw}");

    probe.stop().await;
}

#[tokio::test]
async fn an_idle_session_reports_no_injection_at_all() {
    let probe = Probe::start().await;
    probe.seed_session(&probe.managed_root(), SESSION_ID, &probe.workspace());

    let detail = probe.get_json(&format!("/api/sessions/claude/{SESSION_ID}")).await;
    assert_eq!(detail["injection"], Value::Null, "a session remote-hand never drove has no injection");
    assert_eq!(detail["summary"]["scope"], "owned");
    assert_eq!(detail["canSend"], true);
    assert_eq!(detail["sendBlockReason"], Value::Null);
    assert_eq!(detail["claudeRoot"], "managed");
    assert_eq!(detail["account"], Value::Null);

    probe.stop().await;
}

// --- transcript quality: checklist, context usage, recent directories --------

/// Seed a claude transcript dated `created`, working in `cwd`, whose file mtime
/// is pinned to `mtime_secs`.
///
/// Pinning the mtime is the point: "started" and "last active" have to be able
/// to disagree, and letting the filesystem date the writes would make the order
/// depend on how fast the test ran.
fn seed_dated_session(root: &std::path::Path, id: &str, cwd: &str, created: &str, mtime_secs: u64) {
    let path = common::seed_transcript(
        root,
        "-workspace",
        id,
        &[json!({
            "type": "user",
            "cwd": cwd,
            "timestamp": created,
            "message": {"content": [{"type": "text", "text": "hello"}]},
        })],
    );
    common::set_mtime(&path, mtime_secs);
}

/// A session id that is a plain uuid, distinguished by `n`.
fn session_id(n: usize) -> String {
    format!("aaaaaaaa-1111-4111-8111-{n:012}")
}

#[tokio::test]
async fn recent_directories_rank_by_when_work_started_not_by_last_activity() {
    let probe = Probe::start().await;
    let root = probe.local_root();
    // beta was started most recently but has been quiet since; alpha was started
    // earlier and is still being appended to. Ranking by activity would put
    // alpha first, which is the answer this route deliberately does not give.
    seed_dated_session(&root, &session_id(1), "/w/beta", "2026-06-01T00:00:00.000Z", 1_000);
    seed_dated_session(&root, &session_id(2), "/w/alpha", "2026-01-01T00:00:00.000Z", 9_000);
    seed_dated_session(&root, &session_id(3), "/w/alpha", "2026-03-01T00:00:00.000Z", 8_000);

    // `scope=all`: the fixtures are in the user's own root, and the picker
    // deliberately looks at every scope — see `sessions::recent_cwds`.
    let listed = probe.get_json("/api/sessions?scope=all").await;
    assert_eq!(listed["sessions"][0]["cwd"], "/w/alpha", "by activity, alpha leads");

    let body = probe.get_json("/api/sessions/recent-cwds").await;
    assert_eq!(
        body["cwds"],
        json!(["/w/beta", "/w/alpha"]),
        "one entry per directory, newest *start* first"
    );

    // Restricting to one CLI still answers; codex has nothing seeded here.
    assert_eq!(probe.get_json("/api/sessions/recent-cwds?source=claude").await["cwds"], body["cwds"]);
    assert_eq!(probe.get_json("/api/sessions/recent-cwds?source=codex").await["cwds"], json!([]));

    probe.stop().await;
}

#[tokio::test]
async fn recent_directories_stop_at_eight() {
    let probe = Probe::start().await;
    let root = probe.local_root();
    // Twelve directories, started in ascending order, so the newest eight run
    // from 12 down to 5 and the oldest four must not appear.
    for n in 1..=12 {
        let created = format!("2026-01-{n:02}T00:00:00.000Z");
        seed_dated_session(&root, &session_id(n), &format!("/w/dir{n:02}"), &created, 1_000);
    }

    let cwds: Vec<String> = serde_json::from_value(probe.get_json("/api/sessions/recent-cwds").await["cwds"].clone()).expect("a list of paths");
    assert_eq!(cwds.len(), 8);
    assert_eq!(cwds[0], "/w/dir12");
    assert_eq!(cwds[7], "/w/dir05");
    assert!(!cwds.contains(&"/w/dir04".to_string()), "the ninth-newest is over the cap");

    probe.stop().await;
}

#[tokio::test]
async fn a_sessions_checklist_comes_from_the_task_sidecars() {
    let probe = Probe::start().await;
    probe.seed_session(&probe.local_root(), SESSION_ID, &probe.workspace());

    let tasks = probe.local_root().join("tasks").join(SESSION_ID);
    std::fs::create_dir_all(&tasks).expect("tasks dir");
    // The CLI's own bookkeeping shares the directory with the tasks.
    std::fs::write(tasks.join(".lock"), "").expect("lock");
    std::fs::write(tasks.join(".highwatermark"), "10").expect("highwatermark");
    for (name, id, subject, status) in [
        ("1.json", "1", "first task", "completed"),
        ("2.json", "2", "second task", "in_progress"),
        ("10.json", "10", "tenth task", "pending"),
    ] {
        let task = json!({
            "id": id,
            "subject": subject,
            "description": "the long form",
            "activeForm": "doing it",
            "status": status,
            "blocks": [],
            "blockedBy": [],
        });
        std::fs::write(tasks.join(name), task.to_string()).expect("task");
    }

    let checklist = probe.get_json(&format!("/api/sessions/claude/{SESSION_ID}/checklist")).await;
    assert_eq!(checklist["total"], 3);
    assert_eq!(checklist["done"], 1);
    assert_eq!(checklist["items"][0]["title"], "first task", "`subject` is what the UI calls a title");
    assert_eq!(checklist["items"][1]["status"], "in_progress");
    assert_eq!(checklist["items"][2]["title"], "tenth task", "numbered, not ordered as text");

    probe.stop().await;
}

#[tokio::test]
async fn a_session_with_no_sidecars_has_an_empty_checklist_and_codex_has_none_at_all() {
    let probe = Probe::start().await;
    probe.seed_session(&probe.local_root(), SESSION_ID, &probe.workspace());

    let checklist = probe.get_json(&format!("/api/sessions/claude/{SESSION_ID}/checklist")).await;
    assert_eq!(checklist["items"], json!([]));
    assert_eq!((checklist["total"].as_u64(), checklist["done"].as_u64()), (Some(0), Some(0)));

    let (status, _) = probe.get(&format!("/api/sessions/codex/{SESSION_ID}/checklist")).await;
    assert_eq!(status, 404, "codex keeps no checklists");
    let (status, _) = probe.get("/api/sessions/claude/does-not-exist/checklist").await;
    assert_eq!(status, 404);

    probe.stop().await;
}

#[tokio::test]
async fn the_detail_reports_context_occupancy_against_the_models_own_window() {
    let probe = Probe::start().await;
    let path = probe.seed_session(&probe.local_root(), SESSION_ID, &probe.workspace());
    let turn = json!({
        "type": "assistant",
        "timestamp": "2026-08-27T10:00:00.000Z",
        "message": {
            "role": "assistant",
            "model": "claude-fable-5",
            "content": [{"type": "text", "text": "done"}],
            "usage": {"input_tokens": 2, "cache_read_input_tokens": 878_645, "cache_creation_input_tokens": 259, "output_tokens": 40},
        },
    });
    support::append_line(&path, &turn.to_string());

    let detail = probe.get_json(&format!("/api/sessions/claude/{SESSION_ID}")).await;
    assert_eq!(
        detail["contextTokens"], 878_906,
        "fresh input, cache reads and cache writes all occupy the window"
    );
    assert_eq!(detail["contextWindow"], 1_000_000, "a fable model is a long-context one");

    probe.stop().await;
}

#[tokio::test]
async fn a_session_that_has_never_run_has_no_context_reading() {
    let probe = Probe::start().await;
    probe.seed_session(&probe.local_root(), SESSION_ID, &probe.workspace());

    let detail = probe.get_json(&format!("/api/sessions/claude/{SESSION_ID}")).await;
    assert_eq!(detail["contextTokens"], Value::Null);
    assert_eq!(detail["contextWindow"], 200_000, "an unknown model gets the ordinary window");

    probe.stop().await;
}
