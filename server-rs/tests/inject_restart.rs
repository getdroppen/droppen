//! A CLI run outlives the server that started it, and the next server finds it.
//!
//! The mechanism has three parts and this file tests each at the level it
//! actually has to work at:
//!
//! * **boot adoption** — a `runs` row and a live pid, seeded directly in the
//!   instance's `state.db`, and a server booted onto them. The run is
//!   registered again, the composer stays blocked, and the state clears when
//!   the process exits. Seeding by hand rather than through a first server is
//!   deliberate: two servers in one test process cannot simulate a restart,
//!   because the first one's `flock` descriptor and its `wait()` task are both
//!   still alive.
//! * **the pid-reuse guard** — the same, with a live pid that is not the
//!   process the record named.
//! * **the real thing** — an actual server binary, `SIGKILL`ed through its
//!   process group mid-run, and a second invocation that picks the run up. This
//!   is the test that says the mechanism works rather than that the code agrees
//!   with itself.
//! * **two instances** — two servers, running at once, on one set of sessions.

mod support;

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use support::{plan_slow_cli, Probe};

const SESSION: &str = "dddddddd-4444-4444-8444-dddddddddddd";

// ------------------------------------------------------------------ fixtures

/// A live process the test owns, standing in for a CLI a dead server spawned.
///
/// `/bin/sh` rather than the fake `claude`, because what adoption probes is a
/// pid: nothing about the recovered run reads the program's name, argv or
/// output, and a shell script the test writes can be told exactly when to
/// touch the transcript and exactly when to exit.
struct Stand {
    pid: u32,
    child: Option<std::process::Child>,
}

impl Stand {
    /// Run `script` under `/bin/sh` and return once it has a pid.
    fn start(script: &str) -> Stand {
        let child = std::process::Command::new("/bin/sh")
            .args(["-c", script])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn a stand-in CLI");
        Stand {
            pid: child.id(),
            child: Some(child),
        }
    }

    fn wait(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.wait();
        }
    }
}

impl Drop for Stand {
    /// Never leave a stray process on the developer's machine, whatever the
    /// test did or failed to do.
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Put one in-flight run in the instance's journal.
///
/// Spelled out as JSON rather than built through the server's own types on
/// purpose: the `payload` column is the record's serde encoding, and a test
/// that wrote it with the same code that reads it would prove nothing about
/// that shape.
fn seed_run(db: &Path, session: &str, pid: u32, pid_token: Option<&str>) {
    let mut record = json!({
        "run_id": "run-under-test",
        "source": "claude",
        "session_id": session,
        "cwd": "/tmp",
        "cli_pid": pid,
        "started_at": "2026-01-01T00:00:00.000Z",
    });
    if let Some(token) = pid_token {
        record["pid_token"] = json!(token);
    }
    remote_hand_server::db::at(db)
        .write(|tx| {
            tx.execute(
                "INSERT INTO runs (run_id, session_id, ordinal, payload) VALUES (?1, ?2, 0, ?3)",
                ("run-under-test", session, record.to_string()),
            )
        })
        .expect("seed the run journal");
}

/// Append a completed assistant turn, so the transcript no longer reads as
/// having been cut off mid-turn.
fn append_assistant_turn(transcript: &Path) {
    support::append_line(
        transcript,
        &json!({
            "type": "assistant",
            "timestamp": "2024-05-01T10:00:05.000Z",
            "message": {"content": [{"type": "text", "text": "all done"}]},
        })
        .to_string(),
    );
}

async fn detail(probe: &Probe, id: &str) -> Value {
    probe.get_json(&format!("/api/sessions/claude/{id}")).await
}

/// Poll until `check` holds, or fail saying what it was still seeing.
async fn wait_until(probe: &Probe, id: &str, what: &str, check: impl Fn(&Value) -> bool) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let seen = detail(probe, id).await;
        if check(&seen) {
            return seen;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}; last saw injection={} canSend={}",
            seen["injection"],
            seen["canSend"]
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The runs the journal currently holds, in the order it holds them.
///
/// The `runs` table of the instance's `state.db`.
fn recorded_runs(db: &Path) -> Vec<Value> {
    support::common::json_rows(db, "SELECT payload FROM runs ORDER BY ordinal", ())
}

/// Every lock file sitting in the shared claim directory.
fn claims(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".lock"))
        .collect();
    names.sort();
    names
}

// ------------------------------------------------------------ boot adoption

/// The headline: a run that outlived its server is picked up, blocks the
/// composer while it lasts, and releases it when the CLI finally exits.
#[tokio::test]
async fn a_run_that_outlived_its_server_is_adopted_and_keeps_the_composer_blocked() {
    // Built before the server so the record can name a pid that already exists.
    let home = tempfile::tempdir().expect("tempdir");
    let workspace = home.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    // The instance's own root: a run this server is meant to adopt and be able
    // to send to is, by definition, one of its own sessions.
    let transcript = support::owned_claude_root(home.path())
        .join("projects/-workspace")
        .join(format!("{SESSION}.jsonl"));
    std::fs::create_dir_all(transcript.parent().expect("parent")).expect("project dir");
    std::fs::write(
        &transcript,
        format!(
            "{}\n",
            json!({
                "type": "user",
                "cwd": workspace.to_string_lossy(),
                "timestamp": "2024-05-01T10:00:00.000Z",
                "message": {"content": [{"type": "text", "text": "do the thing"}]},
            })
        ),
    )
    .expect("seed transcript");

    // A stand-in that keeps writing to the transcript, then finishes — the two
    // things a real `claude --print` does that adoption has to keep up with.
    let done = home.path().join("stand.done");
    let mut stand = Stand::start(&format!(
        "sleep 0.6; printf '%s\\n' '{}' >> {}; sleep 0.6; : > {}",
        json!({
            "type": "assistant",
            "timestamp": "2024-05-01T10:00:03.000Z",
            "message": {"content": [{"type": "text", "text": "still working"}]},
        }),
        transcript.display(),
        done.display(),
    ));

    let instance = home.path().join(".local/share/remote-hand/instances/default");
    seed_run(&instance.join("state.db"), SESSION, stand.pid, None);

    let probe = Probe::start_at(home.path()).await;

    // Adopted: the run is reported as running, with the time the *record*
    // carried rather than the moment this server booted.
    let seen = detail(&probe, SESSION).await;
    assert_eq!(seen["injection"]["state"], "running", "the run must be adopted: {seen}");
    assert_eq!(seen["injection"]["startedAt"], "2026-01-01T00:00:00.000Z");
    assert_eq!(seen["injection"]["error"], Value::Null, "an adopted run is not a failure");
    assert_eq!(seen["canSend"], false, "the composer stays blocked while the CLI works");
    assert_eq!(seen["sendBlockReason"], "a message from Droppen is still running");

    // And no second CLI can be started against it.
    let (status, body) = probe
        .send("POST", &format!("/api/sessions/claude/{SESSION}/message"), json!({"text": "again"}))
        .await;
    assert_eq!(status, 409, "a send during an adopted run must be refused: {body}");
    assert_eq!(
        body["error"], "a message from Droppen is still running",
        "the adopted run is in the map, so the route's own sendability gate refuses first — the \
         claim behind it is what stops a *different* server, which the two-instance test covers"
    );

    // The transcript keeps flowing: the CLI is writing, and the server is
    // reading the same file it always did.
    wait_until(&probe, SESSION, "the adopted run's output to reach the transcript", |seen| {
        seen["items"]
            .as_array()
            .expect("items")
            .iter()
            .any(|item| item["text"].as_str().is_some_and(|text| text.contains("still working")))
    })
    .await;

    // When the CLI finally exits, the state clears exactly as a normal
    // completion would — no `Child` to wait on, so the pid poll is what notices.
    stand.wait();
    let seen = wait_until(&probe, SESSION, "the adopted run to finish", |seen| {
        seen["injection"]["state"] != "running"
    })
    .await;
    assert_eq!(seen["injection"]["state"], "ok", "a run that simply ended is not a failure: {seen}");
    assert_eq!(seen["injection"]["error"], Value::Null);
    assert_ne!(seen["injection"]["finishedAt"], Value::Null);
    assert_eq!(seen["canSend"], true, "and the composer is usable again");

    // Nothing is left behind: the record is gone and so is the claim.
    assert!(recorded_runs(&instance.join("state.db")).is_empty(), "a finished run leaves no record");
    assert!(claims(&probe.lock_dir()).is_empty(), "and no claim");

    probe.stop().await;
}

/// A record whose process is gone is cleaned up, and says so — once.
#[tokio::test]
async fn a_run_killed_with_its_server_is_cleaned_up_and_reported_softly() {
    let home = tempfile::tempdir().expect("tempdir");
    let workspace = home.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");

    // A pid that is certainly dead: spawned and reaped before anything reads it.
    let mut corpse = Stand::start("exit 0");
    let dead_pid = corpse.pid;
    corpse.wait();

    // The instance's own root: a run this server is meant to adopt and be able
    // to send to is, by definition, one of its own sessions.
    let transcript = support::owned_claude_root(home.path())
        .join("projects/-workspace")
        .join(format!("{SESSION}.jsonl"));
    std::fs::create_dir_all(transcript.parent().expect("parent")).expect("project dir");
    // Ends on the user's message: the turn never got its reply.
    std::fs::write(
        &transcript,
        format!(
            "{}\n",
            json!({
                "type": "user",
                "cwd": workspace.to_string_lossy(),
                "timestamp": "2024-05-01T10:00:00.000Z",
                "message": {"content": [{"type": "text", "text": "do the thing"}]},
            })
        ),
    )
    .expect("seed transcript");

    let instance = home.path().join(".local/share/remote-hand/instances/default");
    seed_run(&instance.join("state.db"), SESSION, dead_pid, None);

    let probe = Probe::start_at(home.path()).await;

    let seen = detail(&probe, SESSION).await;
    assert_eq!(seen["injection"]["state"], "failed", "a truncated turn is surfaced: {seen}");
    assert_eq!(
        seen["injection"]["error"], "server restarted during this run",
        "the existing failure path carries a soft note rather than a new UI state"
    );
    assert_eq!(seen["injection"]["interrupted"], false);
    assert_eq!(seen["canSend"], true, "but nothing is running, so the composer is free");

    // The record is gone, and no claim was leaked on the way.
    assert!(recorded_runs(&instance.join("state.db")).is_empty(), "a dead run leaves no record");
    assert!(claims(&probe.lock_dir()).is_empty(), "a dead run must not leave a claim");

    // And the session is genuinely usable: the soft note is history, not a lock.
    plan_slow_cli(&workspace, 0.1);
    let (status, body) = probe
        .send("POST", &format!("/api/sessions/claude/{SESSION}/message"), json!({"text": "retry"}))
        .await;
    assert_eq!(status, 200, "{body}");
    probe.wait_for_injection("claude", SESSION).await;

    probe.stop().await;
}

/// A run whose turn completed while the server was down says nothing at all.
#[tokio::test]
async fn a_run_that_finished_while_the_server_was_down_is_reported_as_nothing() {
    let home = tempfile::tempdir().expect("tempdir");
    let workspace = home.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");

    let mut corpse = Stand::start("exit 0");
    let dead_pid = corpse.pid;
    corpse.wait();

    // The instance's own root: a run this server is meant to adopt and be able
    // to send to is, by definition, one of its own sessions.
    let transcript = support::owned_claude_root(home.path())
        .join("projects/-workspace")
        .join(format!("{SESSION}.jsonl"));
    std::fs::create_dir_all(transcript.parent().expect("parent")).expect("project dir");
    std::fs::write(
        &transcript,
        format!(
            "{}\n",
            json!({
                "type": "user",
                "cwd": workspace.to_string_lossy(),
                "timestamp": "2024-05-01T10:00:00.000Z",
                "message": {"content": [{"type": "text", "text": "do the thing"}]},
            })
        ),
    )
    .expect("seed transcript");
    // The CLI got its answer out before it died with the server.
    append_assistant_turn(&transcript);

    seed_run(
        &home.path().join(".local/share/remote-hand/instances/default/state.db"),
        SESSION,
        dead_pid,
        None,
    );

    let probe = Probe::start_at(home.path()).await;
    let seen = detail(&probe, SESSION).await;
    assert_eq!(
        seen["injection"],
        Value::Null,
        "a completed turn needs no explanation; a failure on every restart would be noise: {seen}"
    );
    assert_eq!(seen["canSend"], true);
    probe.stop().await;
}

/// The pid-reuse guard, end to end: the pid is alive and is somebody else.
#[tokio::test]
async fn a_recycled_pid_is_not_mistaken_for_the_run_that_had_it() {
    let home = tempfile::tempdir().expect("tempdir");
    let workspace = home.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");

    // Very much alive — and very much not the process the record describes.
    let stranger = Stand::start("sleep 30");

    // The instance's own root: a run this server is meant to adopt and be able
    // to send to is, by definition, one of its own sessions.
    let transcript = support::owned_claude_root(home.path())
        .join("projects/-workspace")
        .join(format!("{SESSION}.jsonl"));
    std::fs::create_dir_all(transcript.parent().expect("parent")).expect("project dir");
    std::fs::write(
        &transcript,
        format!(
            "{}\n",
            json!({
                "type": "user",
                "cwd": workspace.to_string_lossy(),
                "timestamp": "2024-05-01T10:00:00.000Z",
                "message": {"content": [{"type": "text", "text": "do the thing"}]},
            })
        ),
    )
    .expect("seed transcript");

    // A start-time token from a moment this process cannot have started at.
    seed_run(
        &home.path().join(".local/share/remote-hand/instances/default/state.db"),
        SESSION,
        stranger.pid,
        Some("1.000000"),
    );

    let probe = Probe::start_at(home.path()).await;
    let seen = detail(&probe, SESSION).await;
    assert_ne!(
        seen["injection"]["state"], "running",
        "a live pid alone must not be adopted: the start time has to match too, or a stranger's \
         process gets reported as an agent run and eventually SIGTERMed: {seen}"
    );
    assert_eq!(seen["canSend"], true);
    probe.stop().await;
}

// ----------------------------------------------------------- two instances

/// Two servers, at once, on one set of sessions: the second must not start a
/// second CLI against a session the first is already driving.
///
/// This is the case the in-process map has never been able to see, and the
/// reason the claim lives in the shared data directory rather than in either
/// instance's own.
#[tokio::test]
async fn a_second_instance_cannot_start_a_run_the_first_one_is_already_running() {
    let first = Probe::start().await;
    let workspace = first.workspace();
    first.seed_session(&first.managed_root(), SESSION, &workspace);
    // Long enough that the run is still going when the sibling tries.
    plan_slow_cli(&workspace, 5.0);

    let (status, body) = first
        .send("POST", &format!("/api/sessions/claude/{SESSION}/message"), json!({"text": "mine"}))
        .await;
    assert_eq!(status, 200, "{body}");
    first.wait_for_spawn(&workspace).await;

    let second = first.sibling("other").await;
    // Both instances hold a copy of this session under the same id — which is
    // what two adoptions of one external transcript produce — so both consider
    // it theirs to drive. Ownership has stopped being the thing that separates
    // them, and the claim in the *shared* data directory is all that is left.
    second.seed_session(&second.managed_root(), SESSION, &workspace);
    let seen = second.get_json(&format!("/api/sessions/claude/{SESSION}")).await;
    assert_eq!(
        seen["injection"],
        Value::Null,
        "the second instance has no in-memory record of the first's run, which is exactly why the \
         claim cannot live in memory: {seen}"
    );

    let (status, body) = second
        .send("POST", &format!("/api/sessions/claude/{SESSION}/message"), json!({"text": "mine too"}))
        .await;
    assert_eq!(status, 409, "the claim must refuse it: {body}");
    let message = body["error"].as_str().expect("error");
    assert!(message.contains("already running"), "{message}");
    assert!(
        message.contains(&format!("pid {}", std::process::id())),
        "and name the holder when it can: {message}"
    );

    // Exactly one CLI ever ran, which is the fact all of this exists to protect.
    assert_eq!(support::call_count(&workspace), 1, "a second CLI must never have been spawned");
    // And exactly one claim is held, by the instance that took it.
    assert_eq!(claims(&first.lock_dir()), vec![format!("claude--{SESSION}.lock")]);

    second.stop().await;
    first.wait_for_injection("claude", SESSION).await;
    first.stop().await;
}

// ------------------------------------------------------- across two processes

/// The real thing: a server binary `SIGKILL`ed mid-run, and the next one
/// picking the run up.
///
/// `process_group(0)` on the server means `kill(-pid, …)` reaches it and
/// everything it did not detach — the `sh -c 'server & kill -9 $!'` shape a
/// crash really has. The CLI survives because it was never in that group's
/// way, and the claim is released because the kernel releases an `flock` when
/// the descriptor holding it is closed, however violently.
///
/// The server is killed holding two messages in the two states an accepted
/// message can be in: one already in claude's hands as the turn in flight, and
/// one waiting behind it. Every turn is held until the test lets it go, so
/// that is exactly where things stand when the kill lands, rather than wherever
/// a clock and the server's own driver happened to have got to.
#[tokio::test]
async fn a_run_survives_its_server_being_sigkilled_and_is_adopted_by_the_next_one() {
    // The test owns the home directory: the probe below only lays it out the
    // way the real binary expects, and the servers under test are the two real
    // binaries spawned afterwards.
    let home = tempfile::tempdir().expect("tempdir");
    let probe = Probe::start_at(home.path()).await;
    let workspace = probe.workspace();
    probe.seed_session(&probe.managed_root(), SESSION, &workspace);
    let lock_dir = probe.lock_dir();
    let journal = probe.instance_db();
    probe.stop().await;

    // Held until released, not slept through: the first turn has to still be
    // running when the server dies and when the next one boots, and has to end
    // when the test says so.
    support::plan_held_cli(&workspace);

    let first = spawn_server(home.path()).await;
    let (status, body) = http_json(
        first.addr,
        "POST",
        &format!("/api/sessions/claude/{SESSION}/message"),
        r#"{"text":"carry on"}"#,
    )
    .await;
    assert_eq!(status, 200, "{body}");

    // Read once, the moment the send is answered, and not polled: the record —
    // the thing the next boot reads — is committed before the `200`, and that
    // is what makes a kill survivable however early it lands.
    let record = only_record(&journal);
    let cli_pid = record["cli_pid"].as_u64().expect("cli_pid") as u32;
    // The CLI is nobody's child once the first server dies, so the test is what
    // has to guarantee it does not outlive the test.
    let _cli_guard = KillGroupOnDrop(cli_pid);
    assert_eq!(record["session_id"], SESSION);
    assert!(record["pid_token"].is_string(), "the pid-reuse guard is recorded: {record}");
    assert!(record["resident"] == true, "a claude run is a resident behind a shim: {record}");
    assert_eq!(claims(&lock_dir), vec![format!("claude--{SESSION}.lock")]);
    // The message is in it, in one of two shapes, and which one this read sees
    // is not the test's to choose. A fresh process is born holding the message
    // as `queued`, and its driver writes it to claude straight away — not after
    // `system:init`, which claude does not send until it has read something (see
    // `Driver::pump`) — moving it from `queued` to the turn mark in one commit as
    // it does. The driver runs alongside the response, so this read can land
    // either side of that commit. Insisting on `queued` is what made this test
    // fail now and then on CI, with `"queued":[]` and `"turn_at":0` in hand.
    let waiting = queued_texts(&record) == ["carry on"] && record["turn_at"].is_null();
    let in_flight = queued_texts(&record).is_empty() && record["turn_at"].is_u64();
    assert!(
        waiting || in_flight,
        "the accepted message is recorded — waiting, or as the turn it opened — not merely held in memory: {record}"
    );

    // Then the turn is under way, in claude's hands rather than only marked.
    wait_for(
        || recorded_runs(&journal).first().is_some_and(|run| run["turn_at"].is_u64()),
        "the first message to be taken into a turn",
    )
    .await;
    assert_eq!(support::wait_for_prompts(&workspace, 1).await, vec!["carry on".to_string()]);

    // A second message, behind the turn the first one opened.
    let (status, body) = http_json(
        first.addr,
        "POST",
        &format!("/api/sessions/claude/{SESSION}/message"),
        r#"{"text":"and then this"}"#,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["queued"], true, "it waits for the turn in flight: {body}");
    // Read once again, the moment the send is answered. This is the message an
    // in-memory queue loses: it has been accepted and nothing has been written
    // to claude for it, possibly for as long as the turn in front of it takes.
    let record = only_record(&journal);
    assert_eq!(
        queued_texts(&record),
        ["and then this"],
        "the message waiting behind the turn is on disk the moment its send is answered: {record}"
    );
    assert!(record["turn_at"].is_u64(), "and so is the turn in front of it: {record}");

    unsafe { libc::kill(-(first.pid as libc::pid_t), libc::SIGKILL) };
    wait_for(|| !alive(first.pid), "the server to die").await;
    assert!(alive(cli_pid), "the CLI must survive a SIGKILL aimed at the server's whole process group");
    // The dead server's claim died with its descriptor — no stale lock, and
    // nothing to detect.
    assert!(
        flock_is_free(&lock_dir.join(format!("claude--{SESSION}.lock"))),
        "a dead holder's flock is released by the kernel"
    );

    let second = spawn_server(home.path()).await;
    // Sampled the moment the second server answers at all, and deliberately: the
    // adoption pass runs before the listener does, so the very first request has
    // to already know that this session has something in flight. A server that
    // published the run only once the shim's replay came back would have a
    // window in which the composer was open over a live turn.
    let seen: Value = serde_json::from_str(&http_get(second.addr, &format!("/api/sessions/claude/{SESSION}")).await).expect("json");
    assert_eq!(seen["injection"]["state"], "running", "the second process adopts the run: {seen}");
    assert_eq!(
        seen["canSend"], true,
        "a resident takes messages while it works — they queue behind the turn rather than being refused: {seen}"
    );

    // The claim is held again — by the new server this time, and by exactly one.
    assert_eq!(claims(&lock_dir), vec![format!("claude--{SESSION}.lock")]);
    assert!(
        !flock_is_free(&lock_dir.join(format!("claude--{SESSION}.lock"))),
        "the adopting server re-took the claim"
    );

    // The message the first server was killed holding is delivered by the second
    // one, to the process the first one started — once the turn in front of it
    // ends, and not into the middle of it. This is the promise the whole feature
    // makes, and the one an in-memory queue could not keep: a send that was
    // answered `200` reaches claude, whatever happens to the server in between.
    assert_eq!(
        queued_texts(&only_record(&journal)),
        ["and then this"],
        "the second server keeps the message it inherited waiting behind the turn it inherited"
    );
    support::release_turn(&workspace);
    let prompts = support::wait_for_prompts(&workspace, 2).await;
    assert_eq!(
        prompts,
        vec!["carry on".to_string(), "and then this".to_string()],
        "the accepted message survived the restart"
    );

    let (status, body) = http_json(
        second.addr,
        "POST",
        &format!("/api/sessions/claude/{SESSION}/message"),
        r#"{"text":"me too"}"#,
    )
    .await;
    assert_eq!(status, 200, "and the adopted resident takes the next message: {body}");
    assert_eq!(
        body["queued"], true,
        "which waits for the turn in flight rather than starting a second run: {body}"
    );
    // One CLI, across both servers — the fact all of this exists to protect.
    //
    // Waited for rather than sampled: the record is written the moment `spawn`
    // returns, which is before the child has necessarily reached its first
    // line, so "how many CLIs ran" is only a stable question once at least one
    // of them has said so.
    wait_for(|| support::call_count(&workspace) >= 1, "the CLI to record its call").await;
    assert_eq!(support::call_count(&workspace), 1, "a second CLI must never have been spawned");

    // Nothing here was given the chance to tidy up after itself, which is the
    // point. The CLI leads a process group of its own — that is what let it
    // outlive the first server — so the group is what has to be killed, or its
    // own `sleep` is left running on the developer's machine.
    unsafe { libc::kill(-(second.pid as libc::pid_t), libc::SIGKILL) };
    unsafe { libc::kill(-(cli_pid as libc::pid_t), libc::SIGKILL) };
    wait_for(|| !alive(cli_pid), "the CLI to be cleaned up").await;
}

/// A server started from the real binary, on a home directory the test owns.
///
/// It kills its own process group on the way out. Without that, a test that
/// fails an assertion between spawning a server and killing it leaves a real
/// server — and the CLI it started — running on the developer's machine, which
/// is a nasty way to learn that an assertion moved.
struct Spawned {
    pid: u32,
    addr: std::net::SocketAddr,
}

impl Drop for Spawned {
    fn drop(&mut self) {
        KillGroupOnDrop(self.pid);
    }
}

/// Kills a process group when it goes out of scope.
///
/// Both the server and the CLI it spawns lead groups of their own — the server
/// because [`spawn_server`] says so, the CLI because that is what lets it
/// outlive the server — so a group signal is what reaches every child they
/// forked.
struct KillGroupOnDrop(u32);

impl Drop for KillGroupOnDrop {
    fn drop(&mut self) {
        // SAFETY: a negative pid names a process group; killing one that has
        // already gone is an ignored `ESRCH`.
        unsafe { libc::kill(-(self.0 as libc::pid_t), libc::SIGKILL) };
    }
}

async fn spawn_server(home: &Path) -> Spawned {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-hand-server"));
    command
        .arg("serve")
        .env("APP_HOME_DIR", home)
        // No login: not a test of authentication (`tests/auth.rs` is).
        .env("APP_AUTH__ENABLED", "false")
        .env("APP_SERVER__PORT", "0")
        .env("APP_SERVER__HOST", "127.0.0.1")
        // The fake `claude` is on this process's PATH, and the child inherits
        // it, which is what makes the spawned server drive a fake CLI too.
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        // A group of its own, so the test can kill the group without killing
        // itself — and so the CLI, which is not in it, demonstrably survives.
        .process_group(0)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(false);
    let mut child = command.spawn().expect("spawn the server binary");
    let pid = child.id().expect("the server has a pid");
    tokio::spawn(async move {
        let _ = child.wait().await;
    });

    let port_file = home.join(".local/share/remote-hand/instances/default/server.port");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(port) = std::fs::read_to_string(&port_file).ok().and_then(|t| t.trim().parse::<u16>().ok()) {
            let candidate: std::net::SocketAddr = ([127, 0, 0, 1], port).into();
            if tokio::net::TcpStream::connect(candidate).await.is_ok() {
                return Spawned { pid, addr: candidate };
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the server never recorded a port it was listening on"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The one record in the journal, as it stands right now.
///
/// Read once rather than polled, because what the test asks of it is what is
/// on disk at the moment a send is answered: the server commits before it says
/// `200`, and a poll would wait for whatever the server got round to next and
/// call that the answer.
fn only_record(db: &Path) -> Value {
    let runs = recorded_runs(db);
    assert_eq!(runs.len(), 1, "exactly one run is recorded in {}: {runs:?}", db.display());
    runs.into_iter().next().expect("one run")
}

/// The texts a record holds as waiting, in order.
fn queued_texts(record: &Value) -> Vec<&str> {
    record["queued"]
        .as_array()
        .unwrap_or_else(|| panic!("a record always says what is queued: {record}"))
        .iter()
        .map(|queued| queued["text"].as_str().expect("a queued message has text"))
        .collect()
}

async fn wait_for(mut done: impl FnMut() -> bool, what: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 delivers nothing; it is the existence probe.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Whether nobody holds this lock file — asked by taking it and letting go.
fn flock_is_free(path: &Path) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(file) = std::fs::File::options().read(true).write(true).create(true).truncate(false).open(path) else {
        return true;
    };
    // SAFETY: an open descriptor this function owns, and a flag pair that takes
    // no further arguments.
    let taken = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    if taken {
        // SAFETY: releasing the lock this call just took.
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    }
    taken
}

/// A one-shot HTTP request against a server in another process.
async fn http_get(addr: std::net::SocketAddr, path: &str) -> String {
    http(addr, "GET", path, None).await.1
}

async fn http_json(addr: std::net::SocketAddr, method: &str, path: &str, body: &str) -> (u16, Value) {
    let (status, raw) = http(addr, method, path, Some(body)).await;
    (
        status,
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("expected JSON ({error}): {raw}")),
    )
}

async fn http(addr: std::net::SocketAddr, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(body) = body {
        request.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    if let Some(body) = body {
        request.push_str(body);
    }
    stream.write_all(request.as_bytes()).await.expect("write");

    let mut response = String::new();
    stream.read_to_string(&mut response).await.expect("read");
    let (head, body) = response.split_once("\r\n\r\n").expect("malformed HTTP response");
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("status line");
    let body = if head.to_lowercase().contains("transfer-encoding: chunked") {
        dechunk(body)
    } else {
        body.to_string()
    };
    (status, body)
}

fn dechunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size, remainder)) = rest.split_once("\r\n") {
        let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if size == 0 || remainder.len() < size {
            break;
        }
        out.push_str(&remainder[..size]);
        rest = remainder[size..].strip_prefix("\r\n").unwrap_or("");
    }
    out
}
