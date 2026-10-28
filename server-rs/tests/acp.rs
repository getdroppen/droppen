//! ACP sessions end to end, against a stand-in ACP agent.
//!
//! The stand-in (`support/fake_acp_agent.py`) speaks the ACP the driver uses and
//! writes down everything it is sent, in the session's working directory. It
//! is configured the way a user configures a real agent: through
//! `PUT /api/acp/agents/{id}`, with an absolute executable, its arguments and a
//! variable of its own — so these tests never touch `PATH`. It gives every
//! session it creates the same id, which is what two different agents may do.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::TestServer;
use serde_json::{json, Value};

const FAKE_AGENT: &str = include_str!("support/fake_acp_agent.py");
/// The id the stand-in gives every session it creates.
const SESSION: &str = "f00dfeed0123456789abcdef01234567";

/// The stand-in, written into `home` and made executable.
fn install_fake(home: &Path) -> PathBuf {
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).expect("bin dir");
    let path = bin.join("fake-agent");
    std::fs::write(&path, FAKE_AGENT).expect("fake agent");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

/// Configure the stand-in as the agent `id`, with one variable of the user's
/// own: `FAKE_AGENT_MARK=<mark>`.
async fn configure_as(server: &TestServer, id: &str, program: &Path, mark: &str) {
    let (status, body) = server
        .json(
            "PUT",
            &format!("/api/acp/agents/{id}"),
            &json!({
                "name": format!("Fake {id}"),
                "command": program.to_string_lossy(),
                "args": ["--acp", "--stdio"],
                "env": format!("FAKE_AGENT_MARK={mark}"),
            }),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let settings: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(settings["id"], id);
    assert_eq!(settings["envNames"], json!(["FAKE_AGENT_MARK"]));
    assert_eq!(settings["name"], format!("Fake {id}"));
    assert!(!body.contains(mark), "a value is never sent back: {body}");
}

/// The stand-in as the agent `fake`.
async fn configure(server: &TestServer, program: &Path) {
    configure_as(server, "fake", program, "from-settings").await;
}

/// The stand-in as the agent `id`, with the variables `env`.
async fn configure_env(server: &TestServer, id: &str, program: &Path, env: &str) {
    let (status, body) = server
        .json(
            "PUT",
            &format!("/api/acp/agents/{id}"),
            &json!({"command": program.to_string_lossy(), "args": ["--acp", "--stdio"], "env": env}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
}

/// Whether the stand-in in `workdir` was sent `method`.
fn was_sent(workdir: &Path, method: &str) -> bool {
    calls(workdir).iter().any(|call| call["method"] == method)
}

/// Start a session on the agent `agent`; this server's id for it.
async fn start(server: &TestServer, agent: &str, workdir: &Path, text: &str) -> String {
    let (status, body) = server
        .json(
            "POST",
            "/api/sessions/new",
            &json!({"source": "acp", "acpAgent": agent, "cwd": workdir, "text": text}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    serde_json::from_str::<Value>(&body).expect("json")["id"]
        .as_str()
        .expect("id")
        .to_string()
}

/// The `session` record a transcript opens with.
fn header(server: &TestServer, agent: &str, id: &str) -> Value {
    let file = server.paths.acp_sessions().join(agent).join(format!("{id}.jsonl"));
    let text = std::fs::read_to_string(&file).unwrap_or_else(|error| panic!("{}: {error}", file.display()));
    serde_json::from_str(text.lines().next().expect("a first line")).expect("json")
}

/// What the stand-in was sent, in order.
fn calls(workdir: &Path) -> Vec<Value> {
    std::fs::read_to_string(workdir.join("agent-calls.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("call"))
        .collect()
}

/// Poll the session's detail until `done` says so.
async fn wait_for(server: &TestServer, id: &str, what: &str, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let (status, body) = server.get(&format!("/api/sessions/acp/{id}")).await;
        if status == 200 {
            let detail: Value = serde_json::from_str(&body).expect("json");
            if done(&detail) {
                return detail;
            }
            assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}: {detail:#}");
        } else {
            assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}: {status} {body}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn texts(detail: &Value) -> Vec<(String, String, String)> {
    detail["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| {
            (
                item["role"].as_str().unwrap_or_default().to_string(),
                item["kind"].as_str().unwrap_or_default().to_string(),
                item["text"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn finished(detail: &Value) -> bool {
    detail["injection"]["state"] != "running" && !detail["injection"].is_null()
}

fn item(role: &str, kind: &str, text: &str) -> (String, String, String) {
    (role.into(), kind.into(), text.into())
}

#[tokio::test]
async fn an_acp_session_is_created_streamed_interrupted_reopened_and_archived() {
    let home = tempfile::tempdir().expect("tempdir");
    let program = install_fake(home.path());
    let workdir = home.path().join("work");
    std::fs::create_dir_all(&workdir).expect("workdir");

    let server = TestServer::start_at(home.path(), None).await;
    configure(&server, &program).await;

    // A new session: this server names it, the agent's own id is kept beside
    // it, and the first turn is on screen.
    let id = start(&server, "fake", &workdir, "hi there").await;
    assert_ne!(id, SESSION, "the id is this server's, not the agent's");
    let recorded = header(&server, "fake", &id);
    assert_eq!(
        (&recorded["id"], &recorded["agent"], &recorded["agentSessionId"]),
        (&json!(id), &json!("fake"), &json!(SESSION))
    );
    let detail = wait_for(&server, &id, "the first turn", finished).await;
    assert_eq!(detail["injection"]["state"], "ok", "{detail:#}");
    assert_eq!(
        texts(&detail),
        vec![
            item("user", "text", "hi there"),
            item("assistant", "thinking", "Thinking about hi there"),
            item("assistant", "tool_use", "echo hi"),
            item("user", "tool_result", "hi"),
            item("assistant", "text", "Hello from the agent"),
        ]
    );
    assert_eq!(detail["summary"]["scope"], "owned");
    assert_eq!(detail["summary"]["acpAgent"], "fake");
    assert_eq!(detail["summary"]["title"], "hi there");
    assert_eq!(detail["canSend"], true);
    assert_eq!(
        (detail["contextTokens"].as_u64(), detail["contextWindow"].as_u64()),
        (Some(4020), Some(128000)),
        "the agent's own usage reading"
    );

    let sent = calls(&workdir);
    let methods: Vec<&str> = sent.iter().map(|call| call["method"].as_str().unwrap_or_default()).collect();
    assert_eq!(methods, vec!["initialize", "session/new", "session/prompt"]);
    assert_eq!(
        sent[1]["params"]["cwd"],
        json!(workdir.canonicalize().unwrap()),
        "the agent runs where it was asked to"
    );
    assert!(
        sent.iter().all(|call| call["mark"] == "from-settings"),
        "the configured environment reaches the agent"
    );

    // It lists with the other sessions, and says which agent it runs on.
    let list = server.get_json("/api/sessions?source=acp").await;
    assert_eq!(list["sessions"][0]["id"], json!(id), "{list:#}");
    assert_eq!(list["sessions"][0]["acpAgent"], "fake", "{list:#}");

    // A second message goes to the same process.
    let (status, body) = server
        .json("POST", &format!("/api/sessions/acp/{id}/message"), &json!({"text": "again"}))
        .await;
    assert_eq!(status, 200, "{body}");
    let detail = wait_for(&server, &id, "the second turn", |detail| texts(detail).len() == 10 && finished(detail)).await;
    assert_eq!(texts(&detail)[5], item("user", "text", "again"));
    assert_eq!(
        calls(&workdir).iter().filter(|call| call["method"] == "initialize").count(),
        1,
        "one process"
    );

    // A turn that does not end on its own is stopped from here.
    server
        .json("POST", &format!("/api/sessions/acp/{id}/message"), &json!({"text": "wait for me"}))
        .await;
    wait_for(&server, &id, "the long turn to start", |detail| detail["injection"]["state"] == "running").await;
    wait_for(&server, &id, "the long turn to stream", |detail| {
        texts(detail).last().is_some_and(|last| last.2 == "Working on it")
    })
    .await;
    let (status, body) = server.json("POST", &format!("/api/sessions/acp/{id}/interrupt"), &json!({})).await;
    assert_eq!(status, 200, "{body}");
    let detail = wait_for(&server, &id, "the interruption", finished).await;
    assert_eq!(detail["injection"]["interrupted"], true, "{detail:#}");
    assert_eq!(texts(&detail).last(), Some(&item("system", "text", "Interrupted")));
    let cancel = calls(&workdir).into_iter().find(|call| call["method"] == "session/cancel").expect("cancelled");
    assert_eq!(cancel["params"]["sessionId"], json!(SESSION), "the agent is spoken to by its own id");

    // A restart loses the process, not the session: the next message reopens
    // it, and the agent's replay is not written over the transcript that has it.
    server.stop().await;
    let server = TestServer::start_at(home.path(), None).await;
    let before = texts(&server.get_json(&format!("/api/sessions/acp/{id}")).await).len();
    let (status, body) = server
        .json("POST", &format!("/api/sessions/acp/{id}/message"), &json!({"text": "still there?"}))
        .await;
    assert_eq!(status, 200, "{body}");
    let detail = wait_for(&server, &id, "the reopened turn", |detail| {
        texts(detail).len() == before + 5 && finished(detail)
    })
    .await;
    assert!(
        !texts(&detail).iter().any(|item| item.2 == "an old answer"),
        "the replay is not duplicated"
    );
    let load = calls(&workdir)
        .into_iter()
        .find(|call| call["method"] == "session/load")
        .expect("reopened with session/load");
    assert_eq!(load["params"]["sessionId"], json!(SESSION));

    // Archived, it leaves the list.
    let (status, body) = server.json("POST", &format!("/api/sessions/acp/{id}/archive"), &json!({})).await;
    assert_eq!(status, 200, "{body}");
    let list = server.get_json("/api/sessions?source=acp").await;
    assert_eq!(list["sessions"], json!([]), "{list:#}");
    server.stop().await;
}

#[tokio::test]
async fn a_session_this_server_lost_is_given_the_agents_own_history() {
    let home = tempfile::tempdir().expect("tempdir");
    let program = install_fake(home.path());
    let workdir = home.path().join("work");
    std::fs::create_dir_all(&workdir).expect("workdir");
    let server = TestServer::start_at(home.path(), None).await;
    configure(&server, &program).await;

    // A transcript with nothing in it but which session it is and where it
    // ran: what is left when the conversation this server wrote down is gone.
    let id = "9b2f61c4-56f1-4c1e-9a4d-0d1f2e3c4b5a";
    let dir = server.paths.acp_sessions().join("fake");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(
        dir.join(format!("{id}.jsonl")),
        format!(
            "{}\n",
            json!({"type": "session", "id": id, "agent": "fake", "agentSessionId": SESSION, "cwd": workdir, "timestamp": "2026-10-01T10:00:00.000Z"})
        ),
    )
    .expect("transcript");

    let (status, body) = server
        .json("POST", &format!("/api/sessions/acp/{id}/message"), &json!({"text": "remember?"}))
        .await;
    assert_eq!(status, 200, "{body}");
    let detail = wait_for(&server, id, "the turn", finished).await;
    let texts = texts(&detail);
    assert_eq!(texts[0], item("user", "text", "an old question"));
    assert_eq!(texts[1], item("assistant", "text", "an old answer"));
    assert_eq!(texts[2], item("user", "text", "remember?"));
    let load = calls(&workdir).into_iter().find(|call| call["method"] == "session/load").expect("reopened");
    assert_eq!(load["params"]["sessionId"], json!(SESSION), "reopened by the agent's id");
    server.stop().await;
}

#[tokio::test]
async fn what_the_agent_cannot_do_is_refused_before_it_starts() {
    let home = tempfile::tempdir().expect("tempdir");
    let workdir = home.path().join("work");
    std::fs::create_dir_all(&workdir).expect("workdir");
    let server = TestServer::start_at(home.path(), None).await;

    // A start that names no agent, or one that is not configured, is refused
    // before anything runs, and says why.
    let (status, body) = server
        .json("POST", "/api/sessions/new", &json!({"source": "acp", "cwd": workdir, "text": "hi"}))
        .await;
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("acpAgent"), "{body}");
    let (status, body) = server
        .json(
            "POST",
            "/api/sessions/new",
            &json!({"source": "acp", "acpAgent": "fake", "cwd": workdir, "text": "hi"}),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("no ACP agent \\\"fake\\\" is configured"), "{body}");
    for (agent, source) in [("../fake", "acp"), ("fake", "claude")] {
        let (status, body) = server
            .json(
                "POST",
                "/api/sessions/new",
                &json!({"source": source, "acpAgent": agent, "cwd": workdir, "text": "hi"}),
            )
            .await;
        assert_eq!(status, 400, "{agent} on {source}: {body}");
    }

    // So is an executable that is not there.
    let (status, _) = server
        .json(
            "PUT",
            "/api/acp/agents/missing",
            &json!({"name": "Missing agent", "command": home.path().join("no-such-agent").to_string_lossy()}),
        )
        .await;
    assert_eq!(status, 200);
    let (status, body) = server
        .json(
            "POST",
            "/api/sessions/new",
            &json!({"source": "acp", "acpAgent": "missing", "cwd": workdir, "text": "hi"}),
        )
        .await;
    assert_ne!(status, 200, "{body}");
    assert!(body.contains("Missing agent"), "{body}");

    // Settings the agent does not have are not silently dropped.
    let program = install_fake(home.path());
    configure(&server, &program).await;
    let (status, body) = server
        .json(
            "POST",
            "/api/sessions/new",
            &json!({"source": "acp", "acpAgent": "fake", "cwd": workdir, "text": "hi", "permission": "auto"}),
        )
        .await;
    assert_ne!(status, 200, "{body}");
    assert!(calls(&workdir).is_empty(), "nothing was started");

    // And an environment block that would move the child is refused.
    let (status, _) = server.json("PUT", "/api/acp/agents/fake", &json!({"env": "PATH=/tmp"})).await;
    assert_eq!(status, 400);
    server.stop().await;
}

#[tokio::test]
async fn models_and_efforts_come_from_the_agent_and_are_set_before_the_turn() {
    let home = tempfile::tempdir().expect("tempdir");
    let program = install_fake(home.path());
    let workdir = home.path().join("work");
    std::fs::create_dir_all(&workdir).expect("workdir");
    // A probe runs in a scratch directory that is gone afterwards, so the
    // stand-in logs where the test can read it.
    let log = home.path().join("probe-calls.jsonl");
    let server = TestServer::start_at(home.path(), None).await;
    let (status, body) = server
        .json(
            "PUT",
            "/api/acp/agents/fake",
            &json!({
                "command": program.to_string_lossy(),
                "args": ["--acp", "--stdio"],
                "env": format!("FAKE_AGENT_LOG={}", log.display()),
            }),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let log_calls = || -> Vec<String> {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).expect("call")["method"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    };

    // A list is always some agent's.
    let (status, body) = server.get("/api/models?source=acp").await;
    assert_eq!(status, 400, "{body}");
    // The list is the agent's own, read by a probe that leaves no session behind.
    let models = server.get_json("/api/models?source=acp&acpAgent=fake").await;
    assert_eq!(models["source"], "cache", "{models:#}");
    let ids: Vec<&str> = models["models"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["fake:small", "openrouter:vendor/big"]);
    assert_eq!(models["models"][1]["label"], "Big");
    assert_eq!(models["models"][0]["efforts"], json!(["auto", "low", "high"]));
    assert_eq!(models["models"][0]["defaultEffort"], "auto");
    assert_eq!(
        log_calls(),
        vec!["initialize", "session/new", "session/close"],
        "the probe closes what it opened"
    );

    // Asked again, it is remembered rather than probed.
    server.get_json("/api/models?source=acp&acpAgent=fake").await;
    assert_eq!(log_calls().iter().filter(|m| *m == "initialize").count(), 1);

    // A start that picks both sets both, before the turn.
    let (status, body) = server
        .json(
            "POST",
            "/api/sessions/new",
            &json!({"source": "acp", "acpAgent": "fake", "cwd": workdir, "text": "hi", "model": "openrouter:vendor/big", "effort": "high"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let id = serde_json::from_str::<Value>(&body).expect("json")["id"]
        .as_str()
        .expect("id")
        .to_string();
    let detail = wait_for(&server, &id, "the first turn", finished).await;
    assert_eq!(detail["injection"]["state"], "ok", "{detail:#}");
    assert_eq!(detail["cliChoice"]["model"], "openrouter:vendor/big");
    assert_eq!(detail["cliChoice"]["effort"], "high");
    let turn: Vec<(String, String)> = log_calls()
        .into_iter()
        .skip(3)
        .zip(
            std::fs::read_to_string(&log)
                .unwrap()
                .lines()
                .skip(3)
                .map(|line| {
                    let call: Value = serde_json::from_str(line).unwrap();
                    call["params"]["configId"].as_str().unwrap_or_default().to_string()
                })
                .collect::<Vec<_>>(),
        )
        .collect();
    let methods: Vec<&str> = turn.iter().map(|(method, _)| method.as_str()).collect();
    assert_eq!(
        methods,
        vec![
            "initialize",
            "session/new",
            "session/set_config_option",
            "session/set_config_option",
            "session/prompt"
        ]
    );
    assert_eq!((turn[2].1.as_str(), turn[3].1.as_str()), ("model", "reasoning_effort"));

    // Only what changed is set again.
    let before = log_calls().len();
    server
        .json(
            "POST",
            &format!("/api/sessions/acp/{id}/message"),
            &json!({"text": "again", "model": "openrouter:vendor/big", "effort": "low"}),
        )
        .await;
    let detail = wait_for(&server, &id, "the second turn", |d| texts(d).len() >= 4 && finished(d)).await;
    assert_eq!(detail["cliChoice"]["effort"], "low");
    assert_eq!(log_calls()[before..], ["session/set_config_option", "session/prompt"]);
    // A switched session is nobody else's default: a new one still starts on the agent's.
    let models = server.get_json("/api/models?source=acp&acpAgent=fake").await;
    assert_eq!(models["models"][0]["defaultEffort"], "auto", "{models:#}");

    // What the agent does not offer is refused before anything is sent.
    let before = log_calls().len();
    let (status, body) = server
        .json(
            "POST",
            &format!("/api/sessions/acp/{id}/message"),
            &json!({"text": "x", "model": "not-a-model"}),
        )
        .await;
    assert_ne!(status, 200, "{body}");
    assert!(body.contains("not-a-model"), "{body}");
    let (status, body) = server
        .json(
            "POST",
            &format!("/api/sessions/acp/{id}/message"),
            &json!({"text": "x", "permission": "auto"}),
        )
        .await;
    assert_ne!(status, 200, "{body}");
    assert_eq!(log_calls().len(), before);

    // New settings may mean another configuration: the next list is probed afresh.
    server
        .json(
            "PUT",
            "/api/acp/agents/fake",
            &json!({"env": format!("FAKE_AGENT_LOG={}\nFAKE_AGENT_MARK=changed", log.display())}),
        )
        .await;
    let initializes = log_calls().iter().filter(|m| *m == "initialize").count();
    server.get_json("/api/models?source=acp&acpAgent=fake").await;
    assert_eq!(log_calls().iter().filter(|m| *m == "initialize").count(), initializes + 1);
    server.stop().await;
}

/// What the stand-in was sent in `workdir`, as `(method, mark)` pairs.
fn marked_calls(workdir: &Path) -> Vec<(String, String)> {
    calls(workdir)
        .into_iter()
        .map(|call| {
            (
                call["method"].as_str().unwrap_or_default().to_string(),
                call["mark"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn workdirs(home: &Path, names: &[&str]) -> Vec<PathBuf> {
    names
        .iter()
        .map(|name| {
            let dir = home.join(name);
            std::fs::create_dir_all(&dir).expect("workdir");
            dir
        })
        .collect()
}

#[tokio::test]
async fn two_agents_run_at_once_and_their_sessions_never_collide() {
    let home = tempfile::tempdir().expect("tempdir");
    let program = install_fake(home.path());
    let dirs = workdirs(home.path(), &["work-one", "work-two"]);
    let server = TestServer::start_at(home.path(), None).await;
    configure_as(&server, "one", &program, "mark-one").await;
    configure_as(&server, "two", &program, "mark-two").await;

    let agents = server.get_json("/api/acp/agents").await;
    let ids: Vec<&str> = agents["agents"].as_array().unwrap().iter().map(|agent| agent["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["one", "two"], "{agents:#}");

    // Both agents name their session `SESSION`; this server tells them apart.
    let first = start(&server, "one", &dirs[0], "hello one").await;
    let second = start(&server, "two", &dirs[1], "hello two").await;
    assert_ne!(first, second);
    for (id, agent) in [(&first, "one"), (&second, "two")] {
        assert_eq!(header(&server, agent, id)["agentSessionId"], json!(SESSION));
        let detail = wait_for(&server, id, "the first turn", finished).await;
        assert_eq!(detail["summary"]["acpAgent"], agent, "{detail:#}");
        assert_eq!(texts(&detail)[0], item("user", "text", &format!("hello {agent}")), "each session is its own");
    }
    let list = server.get_json("/api/sessions?source=acp").await;
    let mut rows: Vec<(String, String)> = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row["id"].as_str().unwrap().to_string(), row["acpAgent"].as_str().unwrap().to_string()))
        .collect();
    rows.sort();
    let mut expected = vec![(first.clone(), "one".to_string()), (second.clone(), "two".to_string())];
    expected.sort();
    assert_eq!(rows, expected, "{list:#}");

    // Each agent ran with its own environment, one process each.
    assert!(marked_calls(&dirs[0]).iter().all(|(_, mark)| mark == "mark-one"));
    assert!(marked_calls(&dirs[1]).iter().all(|(_, mark)| mark == "mark-two"));

    // A second message reaches the session it was sent to, and only that one.
    let (status, body) = server
        .json("POST", &format!("/api/sessions/acp/{second}/message"), &json!({"text": "again two"}))
        .await;
    assert_eq!(status, 200, "{body}");
    let detail = wait_for(&server, &second, "the second turn", |detail| texts(detail).len() == 10 && finished(detail)).await;
    assert_eq!(texts(&detail)[5], item("user", "text", "again two"));
    assert_eq!(texts(&server.get_json(&format!("/api/sessions/acp/{first}")).await).len(), 5);

    // Each agent's model list is asked of that agent.
    for agent in ["one", "two"] {
        let models = server.get_json(&format!("/api/models?source=acp&acpAgent={agent}")).await;
        assert_eq!(models["models"].as_array().map(Vec::len), Some(2), "{agent}: {models:#}");
    }
    server.stop().await;
}

#[tokio::test]
async fn old_sessions_keep_their_agent_and_are_reopened_with_it() {
    let home = tempfile::tempdir().expect("tempdir");
    let program = install_fake(home.path());
    let dirs = workdirs(home.path(), &["work-one", "work-two"]);
    let server = TestServer::start_at(home.path(), None).await;
    configure_as(&server, "one", &program, "mark-one").await;
    let old = start(&server, "one", &dirs[0], "an old session").await;
    wait_for(&server, &old, "the old turn", finished).await;

    // The user moves on to another agent, and changes the first one too.
    configure_as(&server, "two", &program, "mark-two").await;
    let new = start(&server, "two", &dirs[1], "a new session").await;
    wait_for(&server, &new, "the new turn", finished).await;
    let (status, body) = server
        .json("PUT", "/api/acp/agents/one", &json!({"name": "Renamed", "env": "FAKE_AGENT_MARK=mark-one-changed"}))
        .await;
    assert_eq!(status, 200, "{body}");

    // A restart loses both processes; the old session still says which agent it
    // ran on, and its next message reopens it there, by the agent's own id.
    server.stop().await;
    let server = TestServer::start_at(home.path(), None).await;
    let row = server.get_json(&format!("/api/sessions/acp/{old}")).await;
    assert_eq!(row["summary"]["acpAgent"], "one", "{row:#}");
    assert_eq!(row["canSend"], true, "{row:#}");
    let (status, body) = server
        .json("POST", &format!("/api/sessions/acp/{old}/message"), &json!({"text": "still you?"}))
        .await;
    assert_eq!(status, 200, "{body}");
    wait_for(&server, &old, "the reopened turn", |detail| texts(detail).len() == 10 && finished(detail)).await;
    let reopened = calls(&dirs[0]).into_iter().filter(|call| call["method"] == "session/load").collect::<Vec<_>>();
    assert_eq!(reopened.len(), 1, "reopened once");
    assert_eq!(reopened[0]["params"]["sessionId"], json!(SESSION));
    assert_eq!(reopened[0]["mark"], "mark-one-changed", "the session's own agent, as it is configured now");
    assert!(
        marked_calls(&dirs[0]).iter().all(|(_, mark)| mark.starts_with("mark-one")),
        "never the other agent: {:?}",
        marked_calls(&dirs[0])
    );
    assert!(marked_calls(&dirs[1]).iter().all(|(_, mark)| mark == "mark-two"));
    let new_row = server.get_json(&format!("/api/sessions/acp/{new}")).await;
    assert_eq!(new_row["summary"]["acpAgent"], "two");
    server.stop().await;
}

#[tokio::test]
async fn a_removed_agents_sessions_stay_listed_and_cannot_be_continued() {
    let home = tempfile::tempdir().expect("tempdir");
    let program = install_fake(home.path());
    let workdir = workdirs(home.path(), &["work"]).remove(0);
    let server = TestServer::start_at(home.path(), None).await;
    configure_as(&server, "gone", &program, "mark").await;
    let id = start(&server, "gone", &workdir, "before").await;
    wait_for(&server, &id, "the turn", finished).await;

    let (status, body) = server.json("DELETE", "/api/acp/agents/gone", &json!({})).await;
    assert_eq!((status, body.as_str()), (200, r#"{"accepted":true}"#));
    let (status, _) = server.json("DELETE", "/api/acp/agents/gone", &json!({})).await;
    assert_eq!(status, 404, "gone already");
    assert_eq!(server.get_json("/api/acp/agents").await["agents"], json!([]));

    // Still listed, still readable, under the agent it ran on.
    let list = server.get_json("/api/sessions").await;
    let row = list["sessions"].as_array().unwrap().iter().find(|row| row["id"] == json!(id)).expect("still listed");
    assert_eq!(row["acpAgent"], "gone");
    let detail = server.get_json(&format!("/api/sessions/acp/{id}")).await;
    assert_eq!(texts(&detail).len(), 5, "{detail:#}");
    assert_eq!(detail["canSend"], false, "{detail:#}");
    assert!(
        detail["sendBlockReason"].as_str().is_some_and(|reason| reason.contains("no longer configured")),
        "{detail:#}"
    );
    // Nothing is sent to it.
    let (status, body) = server
        .json("POST", &format!("/api/sessions/acp/{id}/message"), &json!({"text": "after"}))
        .await;
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("no longer configured"), "{body}");
    assert_eq!(calls(&workdir).iter().filter(|call| call["method"] == "session/prompt").count(), 1);
    let models = server.get_json("/api/models?source=acp&acpAgent=gone").await;
    assert_eq!((models["models"].clone(), models["source"].clone()), (json!([]), json!("fallback")));

    // Configured again under its id, the agent picks its sessions back up, in
    // a process of the agent as it is configured now.
    configure_as(&server, "gone", &program, "mark-again").await;
    assert_eq!(server.get_json(&format!("/api/sessions/acp/{id}")).await["canSend"], true);
    let (status, body) = server
        .json("POST", &format!("/api/sessions/acp/{id}/message"), &json!({"text": "back"}))
        .await;
    assert_eq!(status, 200, "{body}");
    wait_for(&server, &id, "the turn after", |detail| texts(detail).len() == 10 && finished(detail)).await;
    assert_eq!(
        marked_calls(&workdir)[3..],
        ["initialize", "session/load", "session/prompt"].map(|method| (method.to_string(), "mark-again".to_string()))
    );

    // And a session of a removed agent can still be put away.
    let (status, _) = server.json("DELETE", "/api/acp/agents/gone", &json!({})).await;
    assert_eq!(status, 200);
    let (status, body) = server.json("POST", &format!("/api/sessions/acp/{id}/archive"), &json!({})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(server.get_json("/api/sessions?source=acp").await["sessions"], json!([]));
    server.stop().await;
}

#[tokio::test]
async fn the_agent_routes_report_names_never_values() {
    let home = tempfile::tempdir().expect("tempdir");
    let server = TestServer::start_at(home.path(), None).await;

    // Nothing runs until it is added; the built-ins are offered.
    let listed = server.get_json("/api/acp/agents").await;
    assert_eq!(listed["agents"], json!([]));
    let bub = listed["presets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|preset| preset["id"] == "bub")
        .cloned()
        .expect("a built-in");
    assert_eq!((bub["command"].clone(), bub["args"].clone()), (json!("bub"), json!(["acp"])));

    // A built-in added with only its variables is its description, and the
    // values never come back.
    let (status, body) = server
        .json("PUT", "/api/acp/agents/bub", &json!({"env": "BUB_API_KEY=secret-value\nBUB_MODEL=x"}))
        .await;
    assert_eq!(status, 200, "{body}");
    let added: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        (&added["name"], &added["command"], &added["args"], &added["envNames"]),
        (&bub["name"], &json!("bub"), &json!(["acp"]), &json!(["BUB_API_KEY", "BUB_MODEL"]))
    );
    assert_eq!(added["homepage"], bub["homepage"]);
    let (_, listed) = server.get("/api/acp/agents").await;
    assert!(!body.contains("secret-value") && !listed.contains("secret-value"), "{body} {listed}");

    // What cannot be an agent, or would move its child, is refused.
    for (path, patch) in [
        ("/api/acp/agents/Not_A_Slug", json!({"command": "/x"})),
        ("/api/acp/agents/-x", json!({"command": "/x"})),
        ("/api/acp/agents/new-one", json!({"name": "no executable"})),
        ("/api/acp/agents/bub", json!({"env": "PATH=/tmp"})),
        ("/api/acp/agents/bub", json!({"env": "LD_PRELOAD=/x.so"})),
        ("/api/acp/agents/bub", json!({"command": "a\nb"})),
    ] {
        let (status, body) = server.json("PUT", path, &patch).await;
        assert_eq!(status, 400, "{path} {patch}: {body}");
    }
    let (status, _) = server.json("DELETE", "/api/acp/agents/nobody", &json!({})).await;
    assert_eq!(status, 404);
    let (status, _) = server.get("/api/acp/settings").await;
    assert_ne!(status, 200, "the single-agent route is gone");
    server.stop().await;
}

#[tokio::test]
async fn a_check_says_what_the_agent_is_and_why_it_cannot_start() {
    let home = tempfile::tempdir().expect("tempdir");
    let program = install_fake(home.path());
    let server = TestServer::start_at(home.path(), None).await;

    // A ready agent: what it says it is and can do, and where it was found.
    configure_as(&server, "ready", &program, "mark").await;
    let (status, body) = server.json("POST", "/api/acp/agents/ready/check", &json!({})).await;
    assert_eq!(status, 200, "{body}");
    let check: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(check["state"], "ready", "{check:#}");
    assert_eq!(check["resolvedPath"], json!(program.to_string_lossy()));
    assert_eq!(check["protocolVersion"], 1);
    assert_eq!(check["agentInfo"]["version"], "0.1.0");
    assert_eq!(
        (&check["capabilities"]["loadSession"], &check["capabilities"]["image"], &check["capabilities"]["audio"]),
        (&json!(true), &json!(true), &json!(false))
    );
    assert!(check.get("error").is_none(), "{check:#}");
    // Kept with the agent, and nothing was left running or opened.
    let listed = server.get_json("/api/acp/agents").await;
    assert_eq!(listed["agents"][0]["lastCheck"]["state"], "ready", "{listed:#}");

    // What a ready agent says of itself is redacted as its errors are.
    configure_env(&server, "leaky", &program, "FAKE_AGENT_TITLE=leaky-title-secret").await;
    let (status, body) = server.json("POST", "/api/acp/agents/leaky/check", &json!({})).await;
    assert_eq!(status, 200, "{body}");
    let check: Value = serde_json::from_str(&body).unwrap();
    assert_eq!((check["state"].as_str(), check["agentInfo"]["title"].as_str()), (Some("ready"), Some("[redacted]")), "{check:#}");
    assert!(!server.get("/api/acp/agents").await.1.contains("leaky-title-secret"));

    // An executable that is not there: not found, with the PATH it was looked up on.
    let (status, _) = server
        .json("PUT", "/api/acp/agents/missing", &json!({"command": "no-such-agent-anywhere"}))
        .await;
    assert_eq!(status, 200);
    let (_, body) = server.json("POST", "/api/acp/agents/missing/check", &json!({})).await;
    let check: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(check["state"], "not_found", "{check:#}");
    let error = check["error"].as_str().unwrap();
    assert!(error.contains("no-such-agent-anywhere") && error.contains("PATH="), "{error}");

    // One that starts and dies: failed, with what it said, its values taken out.
    let broken = home.path().join("bin").join("broken-agent");
    std::fs::write(&broken, "#!/bin/sh\necho \"cannot load: token $BROKEN_TOKEN\" >&2\nexit 3\n").unwrap();
    std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (status, _) = server
        .json(
            "PUT",
            "/api/acp/agents/broken",
            &json!({"command": broken.to_string_lossy(), "env": "BROKEN_TOKEN=tok-very-secret"}),
        )
        .await;
    assert_eq!(status, 200);
    let (_, body) = server.json("POST", "/api/acp/agents/broken/check", &json!({})).await;
    let check: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(check["state"], "failed", "{check:#}");
    let error = check["error"].as_str().unwrap();
    assert!(error.contains("cannot load: token"), "the agent's own words: {error}");
    assert!(!body.contains("tok-very-secret"), "a value never comes back: {body}");
    // The list says how it ended; the check's own route says why, as redacted.
    let listed = server.get_json("/api/acp/agents").await;
    let row = listed["agents"].as_array().unwrap().iter().find(|agent| agent["id"] == "broken").cloned().unwrap();
    assert_eq!(row["lastCheck"]["state"], "failed", "{row:#}");
    assert!(row["lastCheck"].get("error").is_none(), "{row:#}");
    let kept = server.get_json("/api/acp/agents/broken/check").await;
    assert_eq!(kept["error"], check["error"], "{kept:#}");
    assert!(!kept.to_string().contains("tok-very-secret"), "{kept:#}");

    // An edit of the command line forgets what the last check found.
    server
        .json("PUT", "/api/acp/agents/broken", &json!({"command": program.to_string_lossy(), "args": ["--acp", "--stdio"]}))
        .await;
    let listed = server.get_json("/api/acp/agents").await;
    let broken = listed["agents"].as_array().unwrap().iter().find(|agent| agent["id"] == "broken").cloned().unwrap();
    assert_eq!(broken["lastCheck"], Value::Null, "{broken:#}");

    let (status, _) = server.json("POST", "/api/acp/agents/nobody/check", &json!({})).await;
    assert_eq!(status, 404);
    server.stop().await;
}

#[tokio::test]
async fn an_agent_is_not_removed_while_one_of_its_turns_runs() {
    let home = tempfile::tempdir().expect("tempdir");
    let program = install_fake(home.path());
    let workdir = workdirs(home.path(), &["work"]).remove(0);
    let server = TestServer::start_at(home.path(), None).await;
    configure_as(&server, "busy", &program, "mark").await;
    let id = start(&server, "busy", &workdir, "hello").await;
    wait_for(&server, &id, "the first turn", finished).await;

    // A turn that does not end on its own.
    server
        .json("POST", &format!("/api/sessions/acp/{id}/message"), &json!({"text": "wait for me"}))
        .await;
    wait_for(&server, &id, "the long turn", |detail| detail["injection"]["state"] == "running").await;
    let (status, body) = server.json("DELETE", "/api/acp/agents/busy", &json!({})).await;
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("running a turn"), "{body}");
    assert_eq!(server.get_json("/api/acp/agents").await["agents"][0]["id"], "busy", "still there");

    // Stopped, it can go; its idle process does not hold it back.
    server.json("POST", &format!("/api/sessions/acp/{id}/interrupt"), &json!({})).await;
    wait_for(&server, &id, "the interruption", finished).await;
    let (status, body) = server.json("DELETE", "/api/acp/agents/busy", &json!({})).await;
    assert_eq!(status, 200, "{body}");
    server.stop().await;
}

#[tokio::test]
async fn an_agent_is_not_removed_while_a_session_on_it_starts() {
    let home = tempfile::tempdir().expect("tempdir");
    let program = install_fake(home.path());
    let workdir = workdirs(home.path(), &["work"]).remove(0);
    let server = TestServer::start_at(home.path(), None).await;
    configure_env(&server, "slow", &program, "FAKE_AGENT_SLOW=session/new=2").await;

    // A removal asked for while the session is being opened waits for the
    // start, and then finds its turn running.
    let starting = start(&server, "slow", &workdir, "wait for me");
    let removing = async {
        common::wait_for("the session to be asked for", Duration::from_secs(20), || was_sent(&workdir, "session/new")).await;
        server.json("DELETE", "/api/acp/agents/slow", &json!({})).await
    };
    let (id, (status, body)) = tokio::join!(starting, removing);
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("running a turn"), "{body}");
    assert_eq!(server.get_json("/api/acp/agents").await["agents"][0]["id"], "slow", "still there");
    assert_eq!(
        server.get_json(&format!("/api/sessions/acp/{id}")).await["injection"]["state"],
        "running"
    );

    server.json("POST", &format!("/api/sessions/acp/{id}/interrupt"), &json!({})).await;
    wait_for(&server, &id, "the interruption", finished).await;
    let (status, body) = server.json("DELETE", "/api/acp/agents/slow", &json!({})).await;
    assert_eq!(status, 200, "{body}");
    server.stop().await;
}

#[tokio::test]
async fn an_agent_is_not_removed_while_a_message_waits_for_its_turn() {
    let home = tempfile::tempdir().expect("tempdir");
    let program = install_fake(home.path());
    let workdir = workdirs(home.path(), &["work"]).remove(0);
    let server = TestServer::start_at(home.path(), None).await;
    // The stand-in answers the model switch and then reads nothing for a
    // while, so a prompt too long for the pipe is not taken: the message is
    // off the queue, its model set, and its turn not started.
    configure_env(&server, "slow", &program, "FAKE_AGENT_DEAF=session/set_config_option=3").await;
    let id = start(&server, "slow", &workdir, "hello").await;
    wait_for(&server, &id, "the first turn", finished).await;

    // Longer than a pipe holds, shorter than a message may be.
    let long = "x".repeat(90_000);
    let (status, body) = server
        .json(
            "POST",
            &format!("/api/sessions/acp/{id}/message"),
            &json!({"text": long, "model": "openrouter:vendor/big"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    common::wait_for("the model switch", Duration::from_secs(20), || was_sent(&workdir, "session/set_config_option")).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let detail = server.get_json(&format!("/api/sessions/acp/{id}")).await;
    assert_eq!(detail["injection"]["state"], "ok", "the last turn, not this one: {detail:#}");
    let (status, body) = server.json("DELETE", "/api/acp/agents/slow", &json!({})).await;
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("message waiting"), "{body}");

    let replies = |detail: &Value| texts(detail).iter().filter(|(_, _, text)| text == "Hello from the agent").count();
    wait_for(&server, &id, "the second turn", |detail| replies(detail) == 2 && finished(detail)).await;
    let (status, body) = server.json("DELETE", "/api/acp/agents/slow", &json!({})).await;
    assert_eq!(status, 200, "{body}");
    server.stop().await;
}

#[tokio::test]
async fn a_changed_command_line_or_environment_reaches_the_next_process() {
    let home = tempfile::tempdir().expect("tempdir");
    let program = install_fake(home.path());
    let workdir = workdirs(home.path(), &["work"]).remove(0);
    let server = TestServer::start_at(home.path(), None).await;
    configure_as(&server, "fake", &program, "mark-one").await;
    let id = start(&server, "fake", &workdir, "hello").await;
    wait_for(&server, &id, "the first turn", finished).await;

    // Another environment: the idle process ends, and the next message starts
    // the agent with the new one, reopening the session.
    let (status, body) = server.json("PUT", "/api/acp/agents/fake", &json!({"env": "FAKE_AGENT_MARK=mark-two"})).await;
    assert_eq!(status, 200, "{body}");
    let send = |text: &'static str| {
        let server = &server;
        let id = id.clone();
        async move {
            let (status, body) = server.json("POST", &format!("/api/sessions/acp/{id}/message"), &json!({ "text": text })).await;
            assert_eq!(status, 200, "{body}");
        }
    };
    send("again").await;
    wait_for(&server, &id, "the second turn", |detail| texts(detail).len() == 10 && finished(detail)).await;
    let pair = |method: &str, mark: &str| (method.to_string(), mark.to_string());
    assert_eq!(
        marked_calls(&workdir)[3..],
        [pair("initialize", "mark-two"), pair("session/load", "mark-two"), pair("session/prompt", "mark-two")]
    );

    // A new name is the same process: it is kept.
    let (status, body) = server.json("PUT", "/api/acp/agents/fake", &json!({"name": "Renamed"})).await;
    assert_eq!(status, 200, "{body}");
    send("third").await;
    wait_for(&server, &id, "the third turn", |detail| texts(detail).len() == 15 && finished(detail)).await;
    assert_eq!(marked_calls(&workdir)[6..], [pair("session/prompt", "mark-two")]);

    // A busy one runs what it had when the edit came, where it started, and
    // takes nothing more; it ends after that.
    let replies = |n: usize| {
        move |detail: &Value| texts(detail).iter().filter(|(_, _, text)| text == "Hello from the agent").count() == n && finished(detail)
    };
    send("wait for me").await;
    wait_for(&server, &id, "the long turn", |detail| detail["injection"]["state"] == "running").await;
    send("queued before the edit").await;
    let (status, body) = server.json("PUT", "/api/acp/agents/fake", &json!({"env": "FAKE_AGENT_MARK=mark-three"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(server.get_json(&format!("/api/sessions/acp/{id}")).await["injection"]["state"], "running", "not cut short");
    let (status, body) = server
        .json("POST", &format!("/api/sessions/acp/{id}/message"), &json!({"text": "after the edit"}))
        .await;
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("restarts with its new settings after this turn"), "{body}");
    server.json("POST", &format!("/api/sessions/acp/{id}/interrupt"), &json!({})).await;
    wait_for(&server, &id, "the queued turn", replies(4)).await;
    send("fourth").await;
    wait_for(&server, &id, "the fourth turn", replies(5)).await;
    let calls = marked_calls(&workdir);
    assert_eq!(
        calls[7..10],
        [pair("session/prompt", "mark-two"), pair("session/cancel", "mark-two"), pair("session/prompt", "mark-two")],
        "the long turn and the message queued before the edit ran where they started"
    );
    assert_eq!(
        calls[calls.len() - 3..],
        [pair("initialize", "mark-three"), pair("session/load", "mark-three"), pair("session/prompt", "mark-three")]
    );
    server.stop().await;
}

/// A model's id goes back to the agent as it is, so the list leaves out one
/// that looks like a key rather than show it altered, and says how many it
/// left out. One that only contains the value of one of the agent's
/// variables — as an id names its provider — is listed.
#[tokio::test]
async fn a_model_is_left_out_only_when_its_id_looks_like_a_key() {
    let home = tempfile::tempdir().expect("tempdir");
    let program = install_fake(home.path());
    let workdir = workdirs(home.path(), &["work"]).remove(0);
    let server = TestServer::start_at(home.path(), None).await;
    configure_env(&server, "fake", &program, "FAKE_AGENT_MARK=openrouter\nFAKE_AGENT_FIRST_MODEL=sk-ant-model-like-a-key").await;

    let models = server.get_json("/api/models?source=acp&acpAgent=fake").await;
    let ids: Vec<&str> = models["models"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["fake:small", "openrouter:vendor/big"], "{models:#}");
    assert_eq!(models["hidden"], 1, "{models:#}");

    // A session that runs on it does not write it down either.
    let id = start(&server, "fake", &workdir, "hi").await;
    let detail = wait_for(&server, &id, "the turn", finished).await;
    assert_eq!(detail["cliChoice"]["effort"], "auto", "{detail:#}");
    assert!(detail["cliChoice"]["model"].is_null(), "{detail:#}");
    let file = server.paths.acp_sessions().join("fake").join(format!("{id}.jsonl"));
    assert!(!std::fs::read_to_string(file).unwrap().contains("sk-ant-model-like-a-key"));

    // An agent whose models all show says nothing of hidden ones.
    configure_env(&server, "plain", &program, "").await;
    let models = server.get_json("/api/models?source=acp&acpAgent=plain").await;
    assert!(models.get("hidden").is_none(), "{models:#}");
    server.stop().await;
}

#[tokio::test]
async fn the_built_in_agents_say_whether_their_executable_is_on_path() {
    let home = tempfile::tempdir().expect("tempdir");
    let server = TestServer::start_at(home.path(), None).await;
    let listed = server.get_json("/api/acp/agents").await;
    for preset in listed["presets"].as_array().unwrap() {
        assert!(preset["onPath"].is_boolean(), "{preset:#}");
    }
    server.stop().await;
}
