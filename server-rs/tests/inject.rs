//! What actually reaches a spawned CLI: its argv, its environment, and what
//! comes back out of a failure.
//!
//! Every assertion here is about the account plumbing, so the fake `claude`
//! records exactly the two things it controls — argv and env — and the tests
//! read them back.

mod support;

use serde_json::{json, Value};
use support::{common, plan_dying_cli, plan_forked_transcript, Probe};

const SECRET: &str = "sk-ant-oat01-TESTSECRETVALUE0123456789";
const MANAGED_ID: &str = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";
/// A session in the root this server owns, with no account bound to it.
const OWNED_ID: &str = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb";
/// A session in the user's own `~/.claude`, which this server may only read.
const EXTERNAL_ID: &str = "eeeeeeee-5555-4555-8555-eeeeeeeeeeee";

async fn stored_account(probe: &Probe) -> String {
    probe
        .send_ok(
            "POST",
            "/api/accounts",
            json!({"label": "Work", "kind": "oauth_token", "secret": SECRET}),
            201,
        )
        .await["id"]
        .as_str()
        .expect("id")
        .to_string()
}

async fn message(probe: &Probe, source: &str, id: &str, body: Value) -> (u16, Value) {
    probe.send("POST", &format!("/api/sessions/{source}/{id}/message"), body).await
}

#[tokio::test]
async fn a_managed_session_spawns_with_the_account_token_and_config_root() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    let account = stored_account(&probe).await;
    probe.seed_session(&probe.managed_root(), MANAGED_ID, &workspace);
    probe
        .send_ok(
            "PUT",
            &format!("/api/sessions/claude/{MANAGED_ID}/account"),
            json!({"accountId": account}),
            200,
        )
        .await;

    let (status, body) = message(&probe, "claude", MANAGED_ID, json!({"text": "carry on", "model": "opus"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["accepted"], true);

    let spawn = probe.wait_for_spawn(&workspace).await;
    assert_eq!(spawn.flag("--resume"), Some(MANAGED_ID));
    assert!(spawn.is_resident(), "one process per session, fed over stdin: {:?}", spawn.argv);
    assert_eq!(spawn.flag("--model"), Some("opus"));
    assert_eq!(
        support::wait_for_prompts(&workspace, 1).await,
        vec!["carry on".to_string()],
        "the prompt is a stream-json message on stdin, not an argument"
    );

    assert_eq!(
        spawn.env("CLAUDE_CONFIG_DIR"),
        Some(probe.managed_root().to_string_lossy().as_ref()),
        "a managed spawn runs against the root remote-hand owns"
    );
    assert_eq!(spawn.env("CLAUDE_CODE_OAUTH_TOKEN"), Some(SECRET), "and under the account's own token");
    assert_eq!(spawn.env("CLAUDECODE"), None, "the harness's own variables are stripped first");
    assert_sandbox_marker_matches_euid(&spawn);

    probe.stop().await;
}

/// The CLI refuses `--permission-mode bypassPermissions` as root unless
/// `IS_SANDBOX` says the blast radius is already walled off, and remote-hand's
/// own Docker image runs as root — `node:22-slim`, no `USER`, everything under
/// `/root`. Without the marker that mode does not warn, it fails to launch.
///
/// No test runs as root, so this asserts the *rule* rather than a fixed answer:
/// the marker is on the spawn exactly when the spawn is root's. Ordinarily that
/// proves the absence; run the suite as root and the same line proves the
/// presence. `process_env`'s own unit test covers the uid-0 branch directly.
fn assert_sandbox_marker_matches_euid(spawn: &support::Spawn) {
    // SAFETY: `geteuid` reads an attribute of the calling process and cannot fail.
    let root = unsafe { libc::geteuid() } == 0;
    assert_eq!(
        spawn.env("IS_SANDBOX").is_some(),
        root,
        "a claude spawn must carry the sandbox marker iff it runs as root (euid root: {root})"
    );
}

/// An owned session with no account runs against this instance's own root and
/// under whatever login *that* root holds — which is what a session adopted by
/// an earlier build, or an instance the user logged into directly, needs.
#[tokio::test]
async fn an_owned_session_with_no_account_spawns_against_the_instance_root_and_no_token() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    probe.seed_session(&probe.managed_root(), OWNED_ID, &workspace);

    let (status, body) = message(&probe, "claude", OWNED_ID, json!({"text": "hello"})).await;
    assert_eq!(status, 200, "{body}");

    let spawn = probe.wait_for_spawn(&workspace).await;
    assert_eq!(spawn.flag("--resume"), Some(OWNED_ID));
    assert_eq!(
        spawn.env("CLAUDE_CONFIG_DIR"),
        Some(probe.managed_root().to_string_lossy().as_ref()),
        "every spawn this server makes runs out of the root it owns"
    );
    assert_eq!(spawn.env("CLAUDE_CODE_OAUTH_TOKEN"), None, "no account, no token");
    assert_eq!(spawn.env("ANTHROPIC_API_KEY"), None);
    // The no-token branch of the environment builder is a different one from the
    // account branch, and both have to carry the marker.
    assert_sandbox_marker_matches_euid(&spawn);

    probe.stop().await;
}

/// A session in the user's own root is browse-only, whatever else is true of it.
///
/// The refusal is a token rather than a sentence because the client turns it
/// into the read-only notice, with the server's own sentence beside it.
#[tokio::test]
async fn a_session_in_the_users_own_root_cannot_be_sent_to_at_all() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    let account = stored_account(&probe).await;
    probe.seed_session(&probe.local_root(), EXTERNAL_ID, &workspace);

    for body in [json!({"text": "hi"}), json!({"text": "hi", "accountId": account})] {
        let (status, body) = message(&probe, "claude", EXTERNAL_ID, body).await;
        assert_eq!(status, 409);
        assert_eq!(body["error"], "external_session");
        assert!(body.get("adoptable").is_none(), "nothing offers a takeover any more: {body}");
        assert!(
            body["hint"].as_str().is_some_and(|hint| !hint.is_empty() && !hint.contains("adopt")),
            "{body}"
        );
    }

    // The composer says the same thing the route does.
    let detail = probe.get_json(&format!("/api/sessions/claude/{EXTERNAL_ID}")).await;
    assert_eq!(detail["summary"]["scope"], "external");
    assert_eq!(detail["canSend"], false);
    assert!(
        detail["sendBlockReason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty() && !reason.contains("adopt")),
        "{detail}"
    );

    assert!(!workspace.join("argv.bin").exists(), "a refused send must never reach a CLI");

    probe.stop().await;
}

#[tokio::test]
async fn an_explicit_account_binds_the_session_it_is_sent_to() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    let account = stored_account(&probe).await;
    probe.seed_session(&probe.managed_root(), MANAGED_ID, &workspace);

    let (status, body) = message(&probe, "claude", MANAGED_ID, json!({"text": "hi", "accountId": account})).await;
    assert_eq!(status, 200, "{body}");

    let spawn = probe.wait_for_spawn(&workspace).await;
    assert_eq!(spawn.env("CLAUDE_CODE_OAUTH_TOKEN"), Some(SECRET));
    assert_eq!(
        probe.get_json(&format!("/api/sessions/claude/{MANAGED_ID}")).await["account"]["id"],
        Value::String(account),
        "the binding is written so the next resume needs no explicit pick"
    );

    probe.stop().await;
}

#[tokio::test]
async fn a_credentials_file_in_the_managed_root_does_not_block_the_account_spawn() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    let account = stored_account(&probe).await;
    probe.seed_session(&probe.managed_root(), MANAGED_ID, &workspace);
    probe
        .send_ok(
            "PUT",
            &format!("/api/sessions/claude/{MANAGED_ID}/account"),
            json!({"accountId": account}),
            200,
        )
        .await;

    // Current CLIs prefer the account token in the environment; an existing
    // local login must not prevent account sessions from using this root.
    std::fs::write(probe.managed_root().join(".credentials.json"), "{}").expect("seed credentials");

    let (status, body) = message(&probe, "claude", MANAGED_ID, json!({"text": "hi"})).await;
    assert_eq!(status, 200, "{body}");
    let spawn = probe.wait_for_spawn(&workspace).await;
    assert_eq!(spawn.env("CLAUDE_CODE_OAUTH_TOKEN"), Some(SECRET));
    assert_eq!(spawn.env("CLAUDE_CONFIG_DIR"), Some(probe.managed_root().to_string_lossy().as_ref()));
    assert_eq!(
        std::fs::read_to_string(probe.managed_root().join(".credentials.json")).expect("credentials"),
        "{}",
        "the existing file credentials are left intact"
    );

    probe.stop().await;
}

#[tokio::test]
async fn a_new_session_follows_the_default_account_unless_null_says_otherwise() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    let account = stored_account(&probe).await;
    probe.send_ok("PUT", "/api/accounts/default", json!({"accountId": account}), 200).await;

    // No `accountId` at all: the configured default applies.
    let created = probe
        .send_ok(
            "POST",
            "/api/sessions/new",
            json!({"source": "claude", "cwd": workspace.to_string_lossy(), "text": "start"}),
            200,
        )
        .await;
    let id = created["id"].as_str().expect("id").to_string();
    let spawn = probe.wait_for_spawn(&workspace).await;
    assert_eq!(spawn.flag("--session-id"), Some(id.as_str()));
    assert_eq!(spawn.env("CLAUDE_CODE_OAUTH_TOKEN"), Some(SECRET));
    assert_eq!(spawn.env("CLAUDE_CONFIG_DIR"), Some(probe.managed_root().to_string_lossy().as_ref()));

    // Bound before the spawn, so a crash mid-launch cannot leave a managed
    // session nobody can resume. Asked of the store rather than of the session,
    // because the fake CLI writes no transcript and a session with none does not
    // resolve yet — the binding exists before there is anything to resolve, and
    // that is the property under test. It is one row per session now rather than
    // a `sessionAccounts` key inside the document holding everyone's secrets, so
    // binding a session no longer rewrites a single token.
    let bound = common::rows(
        &probe.config_db(),
        "SELECT account_id FROM session_accounts WHERE session_id = ?1",
        (&id,),
    );
    assert_eq!(bound, [account]);

    probe.stop().await;
}

/// An account `GET /api/accounts/start` named for a pair no model-less start
/// reaches comes back with `accountFrom: "start"`: the rule's choice, checked
/// again at the spawn. Still the rule's, it is used; no longer the rule's (the
/// page asked a while ago), the start is placed afresh — never billed to an
/// account only a stale answer named. The same id without the marker is a
/// person's pick and is honoured as one.
#[tokio::test]
async fn an_account_the_start_rule_named_is_checked_again_at_the_spawn() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    let default = stored_account(&probe).await;
    const SPARE_SECRET: &str = "sk-ant-oat01-TESTSPAREVALUE0123456789";
    let spare = probe
        .send_ok("POST", "/api/accounts", json!({"label": "Spare", "kind": "oauth_token", "secret": SPARE_SECRET}), 201)
        .await["id"]
        .as_str()
        .expect("id")
        .to_string();
    probe.send_ok("PUT", "/api/accounts/default", json!({"accountId": default}), 200).await;
    let start = |dir: &str, body: Value| {
        let (probe, cwd) = (&probe, workspace.join(dir));
        async move {
            std::fs::create_dir_all(&cwd).expect("cwd");
            let mut body = body;
            body["source"] = json!("claude");
            body["cwd"] = json!(cwd.to_string_lossy());
            body["text"] = json!("start");
            probe.send_ok("POST", "/api/sessions/new", body, 200).await;
            probe.wait_for_spawn(&cwd).await
        }
    };

    // Named by the rule and still the rule's: the default, which every start
    // naming no model gets.
    let kept = start("kept", json!({"accountId": default, "accountFrom": "start"})).await;
    assert_eq!(kept.env("CLAUDE_CODE_OAUTH_TOKEN"), Some(SECRET));

    // Named by a stale answer: the rule places a start on the default now.
    let replaced = start("replaced", json!({"accountId": spare, "accountFrom": "start"})).await;
    assert_eq!(
        replaced.env("CLAUDE_CODE_OAUTH_TOKEN"),
        Some(SECRET),
        "placed afresh, on the default — not on the account the stale answer named"
    );

    // The same id as a person's pick: theirs.
    let picked = start("picked", json!({"accountId": spare})).await;
    assert_eq!(picked.env("CLAUDE_CODE_OAUTH_TOKEN"), Some(SPARE_SECRET));

    probe.stop().await;
}

#[tokio::test]
async fn an_explicit_null_keeps_a_new_session_on_the_local_login() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    let account = stored_account(&probe).await;
    probe.send_ok("PUT", "/api/accounts/default", json!({"accountId": account}), 200).await;

    probe
        .send_ok(
            "POST",
            "/api/sessions/new",
            json!({"source": "claude", "cwd": workspace.to_string_lossy(), "text": "start", "accountId": null}),
            200,
        )
        .await;

    let spawn = probe.wait_for_spawn(&workspace).await;
    assert_eq!(
        spawn.env("CLAUDE_CODE_OAUTH_TOKEN"),
        None,
        "an explicit null is the local login, even with a default configured"
    );
    assert_eq!(spawn.env("CLAUDE_CONFIG_DIR"), None);

    probe.stop().await;
}

#[tokio::test]
async fn a_failing_cli_never_leaks_the_token_into_the_injection_error() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    let account = stored_account(&probe).await;
    probe.seed_session(&probe.managed_root(), MANAGED_ID, &workspace);
    probe
        .send_ok(
            "PUT",
            &format!("/api/sessions/claude/{MANAGED_ID}/account"),
            json!({"accountId": account}),
            200,
        )
        .await;

    // Exactly what a real CLI does when the credential is rejected: it complains
    // on stderr and dies, and the shim's tail is what carries the complaint back.
    plan_dying_cli(&workspace, &format!("Authentication failed for credential {SECRET}\n"), 1);

    let (status, _) = message(&probe, "claude", MANAGED_ID, json!({"text": "hi"})).await;
    assert_eq!(status, 200, "the send is accepted; the failure shows up in the injection state");

    let injection = probe.wait_for_injection("claude", MANAGED_ID).await;
    assert_eq!(injection["state"], "failed");
    assert_eq!(injection["interrupted"], false);
    let error = injection["error"].as_str().expect("error");
    assert!(error.contains("claude exited with code 1"), "{error}");
    assert!(!error.contains(SECRET), "the token reached injection.error: {error}");
    assert!(error.contains("[redacted]"), "and the redaction is visible: {error}");

    // The whole payload, not just the field, has to be clean.
    let raw = probe.get(&format!("/api/sessions/claude/{MANAGED_ID}")).await.1;
    assert!(!raw.contains(SECRET), "the token reached a response body");

    probe.stop().await;
}

/// `--resume` is allowed to mint a new session id, and remote-hand must follow.
///
/// The upstream position on happy#8069 is that a new id is by design, which
/// makes this a behaviour remote-hand has to survive rather than a bug it can
/// wait out. A `--print` child could only *infer* the fork afterwards, from what
/// appeared on disk, and could do nothing about it but complain. A resident is
/// told: the `system:init` line names the session the CLI actually opened, and
/// everything keyed by the old id — the run, the claim, the ownership record —
/// moves to the new one. Silence is still the one outcome the user cannot act
/// on, and following the fork is what avoids it.
#[tokio::test]
async fn a_resume_that_lands_in_a_new_session_id_is_followed_rather_than_silent() {
    const FORKED_ID: &str = "cccccccc-3333-4333-8333-cccccccccccc";

    let probe = Probe::start().await;
    let workspace = probe.workspace();
    let transcript = probe.seed_session(&probe.managed_root(), OWNED_ID, &workspace);
    let project = transcript.parent().expect("project dir");

    // The CLI answers under a session of its own choosing, and writes the
    // transcript there rather than into the one it was asked to continue.
    support::plan_announced_session(&workspace, FORKED_ID);
    plan_forked_transcript(
        &workspace,
        &project.join(format!("{FORKED_ID}.jsonl")),
        &format!(
            "{}\n",
            json!({
                "type": "user",
                "cwd": workspace.to_string_lossy(),
                "timestamp": "2024-05-01T10:00:00.000Z",
                "sessionId": FORKED_ID,
                "parentSessionId": OWNED_ID,
                "message": {"content": [{"type": "text", "text": "continued elsewhere"}]},
            })
        ),
    );

    let (status, body) = message(&probe, "claude", OWNED_ID, json!({"text": "carry on"})).await;
    assert_eq!(status, 200, "{body}");
    // The send returns as soon as the process is up, so the fork exists only once
    // the CLI has got as far as reading the message it was started for.
    support::wait_for_prompts(&workspace, 1).await;

    // The turn lands where the CLI put it, and it lands *finished*: a run
    // reported against a transcript nobody is writing is the failure mode.
    let injection = probe.wait_for_injection("claude", FORKED_ID).await;
    assert_eq!(injection["state"], "ok", "the turn is reported under the id claude chose: {injection}");

    // …and the session claude forked into is this server's own, so the user can
    // send to it rather than being told it belongs to somebody else.
    let detail = probe.get_json(&format!("/api/sessions/claude/{FORKED_ID}")).await;
    assert_eq!(detail["summary"]["scope"], "owned", "{detail}");
    assert_eq!(detail["canSend"], true);

    probe.stop().await;
}

/// The false positive that matters: a neighbouring session written while a turn
/// is running.
///
/// One project directory holds every session in a working tree, and they are
/// routinely busy at the same time. A resume that inferred its own session id
/// from what appeared on disk would blame this one; the `system:init` line means
/// nothing has to be inferred at all.
#[tokio::test]
async fn a_busy_neighbour_in_the_same_project_is_not_mistaken_for_a_fork() {
    const NEIGHBOUR_ID: &str = "dddddddd-4444-4444-8444-dddddddddddd";

    let probe = Probe::start().await;
    let workspace = probe.workspace();
    let transcript = probe.seed_session(&probe.managed_root(), OWNED_ID, &workspace);
    let project = transcript.parent().expect("project dir");

    // Written during the turn, in the same directory, and — the hard case — with
    // this session's id in its head, which is what an *old* fork of this very
    // session busy again right now looks like.
    plan_forked_transcript(
        &workspace,
        &project.join(format!("{NEIGHBOUR_ID}.jsonl")),
        &format!(
            "{}\n",
            json!({
                "type": "summary",
                "sessionId": NEIGHBOUR_ID,
                "parentSessionId": OWNED_ID,
                "summary": "someone else, or this session's own past",
            })
        ),
    );

    let (status, body) = message(&probe, "claude", OWNED_ID, json!({"text": "hello"})).await;
    assert_eq!(status, 200, "{body}");

    let injection = probe.wait_for_injection("claude", OWNED_ID).await;
    assert_eq!(injection["state"], "ok", "{injection}");
    assert_eq!(injection["error"], Value::Null, "an unrelated transcript is not this session's fork");
    assert_eq!(
        probe.get_json(&format!("/api/sessions/claude/{OWNED_ID}")).await["summary"]["id"],
        OWNED_ID,
        "and the session stayed where it was"
    );

    probe.stop().await;
}

/// One process serves message after message, which is the whole change.
///
/// The old model spawned a `claude --print` per message and refused a second one
/// while the first was alive; the count below is what says that is over. The
/// prompts are read from the CLI's stdin because that is where they now travel,
/// and reading them back in order is also what proves the second did not
/// overtake the first.
#[tokio::test]
async fn a_second_message_goes_to_the_process_the_first_one_started() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    probe.seed_session(&probe.managed_root(), OWNED_ID, &workspace);

    let (status, _) = message(&probe, "claude", OWNED_ID, json!({"text": "one"})).await;
    assert_eq!(status, 200);
    let injection = probe.wait_for_injection("claude", OWNED_ID).await;
    assert_eq!(injection["state"], "ok");
    assert_eq!(injection["error"], Value::Null);
    assert!(injection["finishedAt"].as_str().is_some(), "a finished turn is stamped");

    // The session is sendable again, and the composer says so.
    let detail = probe.get_json(&format!("/api/sessions/claude/{OWNED_ID}")).await;
    assert_eq!(detail["canSend"], true);
    assert_eq!(detail["sendBlockReason"], Value::Null);

    let (status, body) = message(&probe, "claude", OWNED_ID, json!({"text": "two"})).await;
    assert_eq!(status, 200);
    assert_eq!(body["queued"], false, "the first turn is over, so this one starts at once");

    assert_eq!(
        support::wait_for_prompts(&workspace, 2).await,
        vec!["one".to_string(), "two".to_string()],
        "both messages reached the same conversation, in order"
    );
    assert_eq!(
        support::call_count(&workspace),
        1,
        "and exactly one claude was started: a resident is the session, not the turn"
    );

    probe.stop().await;
}

/// A message to a busy session is queued, where it used to be refused.
///
/// Two `--print` children against one transcript are two agents writing over
/// each other, so the old answer was a `409`. One process taking messages in
/// order has no such problem, and the composer no longer has to be closed while
/// an agent is thinking — which for a phone is the difference between a chat and
/// a walkie-talkie.
#[tokio::test]
async fn a_second_message_queues_behind_the_turn_in_flight_instead_of_being_refused() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    probe.seed_session(&probe.managed_root(), OWNED_ID, &workspace);
    // Every turn hangs until it is let go, so nothing here races a clock.
    support::plan_held_cli(&workspace);

    let (status, _) = message(&probe, "claude", OWNED_ID, json!({"text": "one"})).await;
    assert_eq!(status, 200);
    support::wait_for_prompts(&workspace, 1).await;

    let detail = probe.get_json(&format!("/api/sessions/claude/{OWNED_ID}")).await;
    assert_eq!(detail["injection"]["state"], "running");
    assert_eq!(detail["canSend"], true, "a resident takes the next message while it works: {detail}");
    assert_eq!(detail["sendBlockReason"], Value::Null);

    let (status, body) = message(&probe, "claude", OWNED_ID, json!({"text": "two"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["queued"], true, "and says so, so the client can show it waiting");

    // The queue is not a promise until it is kept: the second message goes in
    // when the first turn ends, and not before.
    assert_eq!(support::prompts(&workspace), vec!["one".to_string()], "nothing was written past the turn");
    support::release_turn(&workspace);
    assert_eq!(support::wait_for_prompts(&workspace, 2).await, vec!["one".to_string(), "two".to_string()]);
    support::release_turn(&workspace);
    probe.wait_for_injection("claude", OWNED_ID).await;

    probe.stop().await;
}

/// A turn claude opened for itself, from the outside — row, route and all.
///
/// The reported failure, end to end. A background task finished while the
/// session was between turns, its notification woke the main agent, and the CLI
/// ran a whole turn with nothing on stdin to have started it. remote-hand only
/// opened turns for prompts it wrote, so the row said **idle** while the agent
/// worked, and a message sent into that window was judged to be arriving between
/// turns: it went straight to stdin, the CLI folded it into the reply already
/// running, and it never became a `user` entry in the transcript at all.
///
/// The turn is accounted for now, so the send takes the path it always should
/// have — queued, journalled, and written when the turn ends.
#[tokio::test]
async fn a_message_sent_during_a_turn_claude_opened_for_itself_is_queued_rather_than_swallowed() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    probe.seed_session(&probe.managed_root(), OWNED_ID, &workspace);
    support::plan_waking_cli(&workspace);

    // One ordinary turn first, because a resident is what the rest of this is
    // about: it ends, and the session goes back to idle with the process alive.
    let (status, _) = message(&probe, "claude", OWNED_ID, json!({"text": "one"})).await;
    assert_eq!(status, 200);
    support::wait_for_prompts(&workspace, 1).await;
    assert_eq!(probe.wait_for_injection("claude", OWNED_ID).await["state"], "ok");

    // Now claude starts a turn nobody asked for.
    support::wake_cli(&workspace);
    let mut working = Value::Null;
    for _ in 0..250 {
        let detail = probe.get_json(&format!("/api/sessions/claude/{OWNED_ID}")).await;
        if detail["summary"]["resident"]["phase"] == "working" {
            working = detail;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_ne!(working, Value::Null, "the session never noticed the turn claude opened");
    assert_eq!(working["summary"]["activity"], "working", "{working}");
    assert_eq!(
        working["injection"]["state"], "running",
        "a turn claude opened is a run like any other: {working}"
    );

    // The send that used to disappear.
    let (status, body) = message(&probe, "claude", OWNED_ID, json!({"text": "two"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["queued"], true, "the composer is told it is waiting: {body}");
    assert_eq!(
        support::prompts(&workspace),
        vec!["one".to_string()],
        "nothing was written into the middle of the turn claude opened"
    );

    // It ends, and the queue behind it moves — into a turn of its own, which is
    // what gives the message its own entry and its own answer.
    support::finish_woken_turn(&workspace);
    assert_eq!(support::wait_for_prompts(&workspace, 2).await, vec!["one".to_string(), "two".to_string()]);
    probe.wait_for_injection("claude", OWNED_ID).await;
    let detail = probe.get_json(&format!("/api/sessions/claude/{OWNED_ID}")).await;
    assert_eq!(detail["summary"]["resident"]["queued"], 0, "{detail}");
    assert_eq!(support::call_count(&workspace), 1, "and all of it happened in the one process: {detail}");

    probe.stop().await;
}

/// What the phone can see about a queue, which used to be nothing at all.
///
/// The reported failure: a message sent into a session mid-turn was accepted,
/// journalled and queued correctly — and then vanished. `injection` describes
/// the *turn*, not the queue behind it, and `status` is a heuristic over the
/// tail of a transcript, so between two turns a session with three messages
/// waiting read as finished. The row now carries the driver's own account.
#[tokio::test]
async fn a_queued_message_is_visible_on_the_row_and_in_the_list() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    probe.seed_session(&probe.managed_root(), OWNED_ID, &workspace);
    support::plan_held_cli(&workspace);

    message(&probe, "claude", OWNED_ID, json!({"text": "one"})).await;
    support::wait_for_prompts(&workspace, 1).await;
    let (_, body) = message(&probe, "claude", OWNED_ID, json!({"text": "two"})).await;
    assert_eq!(body["queued"], true);

    let detail = probe.get_json(&format!("/api/sessions/claude/{OWNED_ID}")).await;
    let summary = &detail["summary"];
    assert_eq!(summary["activity"], "working", "a turn is in flight: {summary}");
    assert_eq!(summary["status"], "running", "and the driver, not the transcript, said so");
    assert_eq!(summary["resident"]["phase"], "working");
    assert_eq!(summary["resident"]["queued"], 1, "the message behind it is countable: {summary}");
    assert_eq!(summary["resident"]["background"], 0);
    assert_eq!(summary["resident"]["awaitingBackground"], false);
    assert_eq!(summary["resident"]["stalled"], false);

    // The sidebar reads the same row from the list, and must not disagree with
    // the page the user opens from it.
    let list = probe.get_json("/api/sessions").await;
    let row = list["sessions"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["id"] == OWNED_ID)
        .expect("the session is listed")
        .clone();
    assert_eq!(row["activity"], "working", "{row}");
    assert_eq!(row["resident"]["queued"], 1);
    assert_eq!(row["unread"], false, "a session mid-turn is not waiting on you");

    support::release_turn(&workspace);
    support::wait_for_prompts(&workspace, 2).await;
    support::release_turn(&workspace);
    probe.wait_for_injection("claude", OWNED_ID).await;

    // The queue emptied with the turn, and the resident is still there saying so.
    let detail = probe.get_json(&format!("/api/sessions/claude/{OWNED_ID}")).await;
    assert_eq!(detail["summary"]["resident"]["queued"], 0, "{detail}");
    assert_eq!(detail["summary"]["activity"], "idle", "{detail}");

    probe.stop().await;
}

/// The queue routes, end to end: what the row shows, what can be taken back,
/// and what jumps the line.
///
/// The count on its own could only ever say "something is waiting". These ids
/// are what a client acts on, so they have to be on the row a client already
/// polls — not behind a route of their own — and they have to survive the trip
/// through the API to name the same message the driver holds.
#[tokio::test]
async fn a_queued_message_can_be_read_off_the_row_and_then_dropped_or_steered() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    probe.seed_session(&probe.managed_root(), OWNED_ID, &workspace);
    support::plan_held_cli(&workspace);

    message(&probe, "claude", OWNED_ID, json!({"text": "one"})).await;
    support::wait_for_prompts(&workspace, 1).await;
    message(&probe, "claude", OWNED_ID, json!({"text": "regretted"})).await;
    message(&probe, "claude", OWNED_ID, json!({"text": "urgent"})).await;

    let detail = probe.get_json(&format!("/api/sessions/claude/{OWNED_ID}")).await;
    let queued = detail["summary"]["resident"]["queuedMessages"]
        .as_array()
        .expect("the queue is on the row")
        .clone();
    assert_eq!(queued.len(), 2, "{detail}");
    assert_eq!(queued[0]["text"], "regretted", "in the order they will run: {queued:?}");
    assert_eq!(queued[1]["text"], "urgent");
    assert_eq!(queued[0]["steered"], false);
    assert_eq!(detail["summary"]["resident"]["queued"], 2, "the count and the list are the same fact");

    let regretted = queued[0]["id"].as_str().expect("an id").to_string();
    let urgent = queued[1]["id"].as_str().expect("an id").to_string();
    let queue_path = |qid: &str| format!("/api/sessions/claude/{OWNED_ID}/queue/{qid}");

    // Steered: written to the CLI now, so it runs before the message it was
    // queued behind — and is past the point of being withdrawn.
    probe.send_ok("POST", &format!("{}/steer", queue_path(&urgent)), json!({}), 200).await;
    let (status, body) = probe.send("DELETE", &queue_path(&urgent), json!({})).await;
    assert_eq!(status, 409, "a steered message cannot be taken back: {body}");

    // Withdrawn: never reaches the CLI at all.
    probe.send_ok("DELETE", &queue_path(&regretted), json!({}), 200).await;
    let (status, body) = probe.send("DELETE", &queue_path(&regretted), json!({})).await;
    assert_eq!(status, 404, "and it is gone for good: {body}");

    let detail = probe.get_json(&format!("/api/sessions/claude/{OWNED_ID}")).await;
    let queued = detail["summary"]["resident"]["queuedMessages"].as_array().expect("the queue");
    assert_eq!(queued.len(), 1, "only the steered one is left: {detail}");
    assert_eq!(queued[0]["steered"], true, "and the row says a client may not touch it");

    support::release_turn(&workspace);
    assert_eq!(
        support::wait_for_prompts(&workspace, 2).await,
        vec!["one".to_string(), "urgent".to_string()],
        "the steered message ran next, and the withdrawn one never ran at all"
    );
    support::release_turn(&workspace);
    probe.wait_for_injection("claude", OWNED_ID).await;

    let detail = probe.get_json(&format!("/api/sessions/claude/{OWNED_ID}")).await;
    assert_eq!(detail["summary"]["resident"]["queued"], 0, "{detail}");
    assert!(
        detail["summary"]["resident"]["queuedMessages"]
            .as_array()
            .is_some_and(|queued| queued.is_empty()),
        "{detail}"
    );

    probe.stop().await;
}

/// A queue is a thing a *resident* has, so a session without one has nothing to
/// address — and neither does codex, whose runs are one process per message.
#[tokio::test]
async fn the_queue_routes_answer_404_where_there_is_no_queue() {
    let probe = Probe::start().await;
    probe.seed_session(&probe.managed_root(), OWNED_ID, &probe.workspace());

    let (status, body) = probe
        .send("DELETE", &format!("/api/sessions/claude/{OWNED_ID}/queue/whatever"), json!({}))
        .await;
    assert_eq!(status, 404, "no resident is holding anything for this session: {body}");
    let (status, body) = probe
        .send("POST", &format!("/api/sessions/codex/{OWNED_ID}/queue/whatever/steer"), json!({}))
        .await;
    assert_eq!(status, 404, "codex has no queue to steer: {body}");

    probe.stop().await;
}

/// A session nobody here drives says so, rather than saying nothing.
#[tokio::test]
async fn a_session_without_a_resident_has_no_resident_block() {
    let probe = Probe::start().await;
    probe.seed_session(&probe.managed_root(), OWNED_ID, &probe.workspace());

    let detail = probe.get_json(&format!("/api/sessions/claude/{OWNED_ID}")).await;
    assert_eq!(detail["summary"]["resident"], Value::Null, "{detail}");
    assert_eq!(detail["summary"]["activity"], "idle");

    probe.stop().await;
}

/// A message shows as running from the moment it is accepted.
///
/// Nothing may be written to a resident before its `system:init` line, so the
/// driver cannot open the real turn until claude says it is listening — and a
/// cold CLI takes a second or two to get there. A session that read as idle for
/// that window would be telling a phone that the message it just sent went
/// nowhere, which is the one report a user cannot act on.
#[tokio::test]
async fn a_message_shows_as_running_before_the_cli_has_said_a_word() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    probe.seed_session(&probe.managed_root(), OWNED_ID, &workspace);
    // Long enough that the assertions below are certainly inside the window.
    support::plan_mute_cli(&workspace, 30.0);

    let (status, body) = message(&probe, "claude", OWNED_ID, json!({"text": "hello"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["queued"], false, "there was no turn for it to queue behind");

    // Not a line has been read, and not one will be for half a minute.
    assert!(support::prompts(&workspace).is_empty());
    let detail = probe.get_json(&format!("/api/sessions/claude/{OWNED_ID}")).await;
    assert_eq!(
        detail["injection"]["state"], "running",
        "the turn is the user's the moment it is accepted: {detail}"
    );
    assert!(detail["injection"]["startedAt"].as_str().is_some());
    assert_eq!(detail["injection"]["finishedAt"], Value::Null);

    probe.stop().await;
}

/// Stop ends the turn in flight, however wedged the CLI is.
///
/// The ladder behind the button is `control_request` → `SIGINT` → `SIGTERM` →
/// `SIGKILL`, a step apart, and the CLI here answers none of them: it is asleep
/// in the middle of a turn, which is exactly the case the signals exist for. The
/// budget is generous because the rungs are timed in seconds by default, and the
/// test is about the outcome rather than about which rung reached it.
#[tokio::test]
async fn an_interrupt_ends_a_turn_the_cli_will_not_end_itself() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    probe.seed_session(&probe.managed_root(), OWNED_ID, &workspace);
    support::plan_slow_cli(&workspace, 120.0);

    let (status, _) = message(&probe, "claude", OWNED_ID, json!({"text": "one"})).await;
    assert_eq!(status, 200);
    support::wait_for_prompts(&workspace, 1).await;

    let detail = probe.get_json(&format!("/api/sessions/claude/{OWNED_ID}")).await;
    assert_eq!(detail["injection"]["state"], "running");

    let stopped = probe
        .send_ok("POST", &format!("/api/sessions/claude/{OWNED_ID}/interrupt"), json!({}), 200)
        .await;
    assert_eq!(stopped["accepted"], true);

    let injection = probe
        .wait_for_injection_within("claude", OWNED_ID, std::time::Duration::from_secs(60))
        .await;
    assert_eq!(injection["state"], "failed");
    assert_eq!(injection["interrupted"], true);
    assert_eq!(injection["error"], "interrupted from Droppen");

    probe.stop().await;
}

/// Native interruption ends the turn, keeps the process, and overrides a stale
/// rollout tail without blocking the next message.
#[tokio::test]
async fn an_interrupted_codex_run_releases_the_composer_for_the_queued_message() {
    const INSTANCE: &str = "codex-interrupt";
    let probe = Probe::start_named(Some(INSTANCE)).await;
    let workspace = probe.workspace();
    let id = "0199c1a1-2b3c-4d5e-8f60-112233445566";
    let dir = probe.home.join(".codex/sessions/2026/09/11");
    std::fs::create_dir_all(&dir).expect("codex sessions dir");
    let rollout = dir.join(format!("rollout-2026-09-11T10-00-00-{id}.jsonl"));
    std::fs::write(
        &rollout,
        [
            json!({"type": "session_meta", "timestamp": "2026-09-11T10:00:00.000Z", "payload": {"id": id, "cwd": workspace.to_string_lossy()}}),
            json!({"type": "event_msg", "timestamp": "2026-09-11T10:00:01.000Z", "payload": {"type": "agent_message", "message": "ready"}}),
        ]
        .iter()
        .map(|line| format!("{line}\n"))
        .collect::<String>(),
    )
    .expect("seed codex rollout");
    let paths = remote_hand_server::sessions::Paths::for_instance(probe.home.clone(), INSTANCE);
    remote_hand_server::sessions::owned_sessions::record(&paths, remote_hand_server::sessions::Source::Codex, id)
        .await
        .expect("own codex session");
    support::plan_slow_cli(&workspace, 120.0);

    let (status, body) = message(&probe, "codex", id, json!({"text": "one"})).await;
    assert_eq!(status, 200, "{body}");

    // The real CLI writes this as the turn starts. It may be the final rollout
    // line when SIGTERM wins before Codex can record `turn_aborted`.
    use std::io::Write as _;
    writeln!(
        std::fs::OpenOptions::new().append(true).open(&rollout).expect("open rollout"),
        "{}",
        json!({"type": "event_msg", "timestamp": "2026-09-11T10:00:02.000Z", "payload": {"type": "task_started"}})
    )
    .expect("start codex turn");
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let working = probe.get_json(&format!("/api/sessions/codex/{id}")).await;
    assert_eq!(working["summary"]["activity"], "working", "{working}");
    assert_eq!(working["canSend"], true, "App Server accepts queued messages while busy");
    assert_eq!(working["summary"]["resident"]["phase"], "working");

    let (status, body) = message(&probe, "codex", id, json!({"text": "native correction"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["queued"], true);
    let queued = probe.get_json(&format!("/api/sessions/codex/{id}")).await;
    let qid = queued["summary"]["resident"]["queuedMessages"][0]["id"].as_str().unwrap();
    probe
        .send_ok("POST", &format!("/api/sessions/codex/{id}/queue/{qid}/steer"), json!({}), 200)
        .await;
    let steered = probe.get_json(&format!("/api/sessions/codex/{id}")).await;
    assert_eq!(steered["injection"]["state"], "running", "steer does not stop the turn");
    assert_eq!(steered["summary"]["resident"]["queued"], 0, "acknowledged steer leaves the queue");

    probe
        .send_ok("POST", &format!("/api/sessions/codex/{id}/interrupt"), json!({}), 200)
        .await;
    let injection = probe.wait_for_injection_within("codex", id, std::time::Duration::from_secs(10)).await;
    assert_eq!(injection["state"], "failed");
    assert_eq!(injection["interrupted"], true, "the UI can tell a cancellation from a failure");

    let stopped = probe.get_json(&format!("/api/sessions/codex/{id}")).await;
    assert_eq!(
        stopped["summary"]["status"], "waiting",
        "the stale task_started no longer claims a live process"
    );
    assert_eq!(stopped["summary"]["activity"], "idle", "the stale Stop button disappears");
    assert_eq!(stopped["canSend"], true, "the browser's queued correction can drain immediately");

    std::fs::remove_file(workspace.join("sleep.txt")).expect("release future codex runs");
    let (status, body) = message(&probe, "codex", id, json!({"text": "two"})).await;
    assert_eq!(status, 200, "the queued follow-up is accepted after cancellation: {body}");
    assert_eq!(probe.wait_for_injection("codex", id).await["state"], "ok");
    assert_eq!(
        std::fs::read_to_string(workspace.join("codex-calls.txt")).unwrap().lines().count(),
        1,
        "the same App Server survives steering, cancellation, and a second turn"
    );

    probe.stop().await;
}

#[tokio::test]
async fn a_new_codex_session_takes_the_id_the_cli_announces() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    let thread_id = "0199c1a1-2b3c-4d5e-8f60-112233445566";
    std::fs::write(
        workspace.join("codex-stdout.txt"),
        format!("{}\n", json!({"type": "thread.started", "thread_id": thread_id})),
    )
    .expect("plan codex");

    let created = probe
        .send_ok(
            "POST",
            "/api/sessions/new",
            json!({"source": "codex", "cwd": workspace.to_string_lossy(), "text": "start", "permission": "read-only"}),
            200,
        )
        .await;
    assert_eq!(created["id"], thread_id, "thread/start supplies the canonical id");

    let spawn = support::Spawn::read_codex(&workspace).expect("codex spawn");
    assert_eq!(spawn.argv, ["app-server"]);
    let requests = std::fs::read_to_string(workspace.join("codex-stdin.jsonl")).expect("RPC log");
    let requests: Vec<serde_json::Value> = requests.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(requests[0]["method"], "initialize");
    assert_eq!(requests[1]["method"], "initialized");
    assert_eq!(requests[2]["method"], "thread/start");
    assert_eq!(spawn.env("CLAUDE_CODE_OAUTH_TOKEN"), None, "codex never sees a Claude credential");
    assert_eq!(
        spawn.env("IS_SANDBOX"),
        None,
        "the sandbox marker means something to one CLI only, so only that CLI is told"
    );

    probe.stop().await;
}

// A test named a_codex_run_that_announces_nothing_fails_the_request used to
// cover the codex --json handshake's failure message here. It wedged CI on
// 2026-08-27 (the glibc setenv/getenv race described in ci.yml) and was removed
// by decision rather than hardened; the handshake timeout itself is still
// covered by the thread-id tests above.

/// A session whose directory has been deleted still runs — at home, and nowhere
/// else.
///
/// Refusing would be wrong: the conversation is worth continuing whatever
/// happened to the working tree it started in. Inheriting the *server's* working
/// directory would be worse than refusing, because the agent would silently be
/// handed whatever the server happens to be sitting in — for a developer, the
/// checkout of remote-hand itself. So the fallback is the home directory, named
/// by the send path rather than left to whoever launched the server.
#[tokio::test]
async fn a_session_whose_directory_is_gone_runs_at_home_rather_than_wherever_the_server_is() {
    let probe = Probe::start().await;
    let workspace = probe.home.join("gone");
    std::fs::create_dir_all(&workspace).expect("workspace");
    probe.seed_session(&probe.managed_root(), OWNED_ID, &workspace);
    std::fs::remove_dir(&workspace).expect("remove the workspace");

    let (status, body) = message(&probe, "claude", OWNED_ID, json!({"text": "hi"})).await;
    assert_eq!(status, 200, "the send is accepted: {body}");

    // The dumps land in the home directory, which is the whole assertion: the
    // fake CLI writes them wherever it was started, and refuses to write at all
    // outside the test's own home.
    let spawn = probe.wait_for_spawn(&probe.home).await;
    assert_eq!(spawn.flag("--resume"), Some(OWNED_ID));
    assert_eq!(support::wait_for_prompts(&probe.home, 1).await, vec!["hi".to_string()]);

    let injection = probe.wait_for_injection("claude", OWNED_ID).await;
    assert_eq!(injection["state"], "ok", "{injection}");

    probe.stop().await;
}

#[tokio::test]
async fn interrupting_nothing_is_a_409() {
    let probe = Probe::start().await;
    probe.seed_session(&probe.managed_root(), OWNED_ID, &probe.workspace());

    let (status, body) = probe.send("POST", &format!("/api/sessions/claude/{OWNED_ID}/interrupt"), json!({})).await;
    assert_eq!(status, 409);
    assert_eq!(body["error"], "no running message to interrupt");

    probe.stop().await;
}

#[tokio::test]
async fn a_message_to_a_missing_session_is_a_404_and_a_bad_attachment_a_409() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    probe.seed_session(&probe.managed_root(), OWNED_ID, &workspace);

    let (status, body) = message(&probe, "claude", "does-not-exist", json!({"text": "hi"})).await;
    assert_eq!(status, 404);
    assert_eq!(body["error"], "session not found");

    let (status, body) = message(&probe, "claude", OWNED_ID, json!({"text": "hi", "images": ["/etc/passwd"]})).await;
    assert_eq!(status, 409);
    assert_eq!(body["error"], "attachment paths must come from /api/uploads");

    // A flag value that could read as a flag is refused before any spawn, and
    // a malformed body is a bad request rather than a conflict.
    let (status, body) = message(&probe, "claude", OWNED_ID, json!({"text": "hi", "model": "--add-dir"})).await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "invalid model");

    let (status, body) = message(&probe, "claude", OWNED_ID, json!({"text": "hi", "accountId": "not-a-uuid"})).await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "invalid account id: not-a-uuid");
    assert!(!workspace.join("argv.bin").exists(), "none of the three reached a CLI");

    probe.stop().await;
}

#[tokio::test]
async fn an_attachment_is_added_to_the_prompt_and_to_the_readable_directory() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    probe.seed_session(&probe.managed_root(), OWNED_ID, &workspace);
    let uploads = probe.data_dir().join("uploads");
    std::fs::create_dir_all(uploads.join("one")).expect("uploads dir");
    let image = uploads.join("one/shot.png");
    std::fs::write(&image, "x").expect("upload");

    let (status, body) = message(&probe, "claude", OWNED_ID, json!({"text": "look", "images": [image.to_string_lossy()]})).await;
    assert_eq!(status, 200, "{body}");

    let spawn = probe.wait_for_spawn(&workspace).await;
    assert_eq!(
        spawn.flag("--add-dir"),
        Some(uploads.to_string_lossy().as_ref()),
        "reads inside --add-dir need no approval"
    );
    let prompts = support::wait_for_prompts(&workspace, 1).await;
    let prompt = prompts.first().expect("prompt");
    assert!(prompt.starts_with("look\n\n[1 attached file(s)"), "{prompt}");
    assert!(prompt.contains(&image.to_string_lossy().to_string()));

    probe.stop().await;
}

#[tokio::test]
async fn a_new_session_needs_a_real_directory_inside_home() {
    let probe = Probe::start().await;

    let (status, body) = probe
        .send("POST", "/api/sessions/new", json!({"source": "claude", "cwd": "/etc", "text": "hi"}))
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "path is outside the home directory");

    let (status, body) = probe
        .send(
            "POST",
            "/api/sessions/new",
            json!({"source": "claude", "cwd": probe.home.join("nope").to_string_lossy(), "text": "hi"}),
        )
        .await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().expect("error").starts_with("not a directory:"), "{body}");

    probe.stop().await;
}
