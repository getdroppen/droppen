//! State machine, message queue, and watchdog tests.

use super::*;

// ------------------------------------------------------ the state machine

/// A turn claude opens by itself is still a turn.
///
/// The reported bug, from the top: a background task finished, its
/// notification woke the main agent, and the CLI ran a whole turn with
/// nothing on stdin to have started it. The session showed *idle* the whole
/// time, because the phase only ever moved when this server wrote a prompt.
///
/// The three lines that must **not** move it are asserted first, because
/// each of them is a way to get a session stuck working on nothing: a
/// subagent's forwarded stream, a background task's progress, and the woken
/// turn's own `init`.
#[tokio::test]
async fn a_turn_claude_opens_for_itself_takes_the_session_out_of_idle() {
    let harness = Harness::ordinary_with(&wakes_itself_for_a_finished_task()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    // A subagent talking is not this session working. The task list that
    // follows it is the barrier: seeing it move proves the driver has read
    // everything above it.
    harness.nudge("subagent");
    until("the subagent's stream", || harness.state().background == 1).await;
    assert_eq!(
        harness.state().phase,
        Phase::Idle,
        "a forwarded child event is the subagent working, not this thread"
    );

    // Nor is the `init` the woken turn announces itself with — every process
    // writes one of those at start-up.
    harness.nudge("wake");
    until("the woken turn's init", || harness.state().background == 0).await;
    assert_eq!(harness.state().phase, Phase::Idle, "an init is not a turn");

    // The main thread's own first line is.
    harness.nudge("speak");
    until("the turn claude opened", || harness.state().phase == Phase::Working).await;

    // And claude's own `result` ends it, exactly as it ends a turn this
    // server wrote a prompt for.
    harness.nudge("finish");
    until("the result", || harness.state().phase == Phase::Idle).await;

    assert!(
        harness.seen().is_empty(),
        "nothing was written to claude for a turn it started itself: {:?}",
        harness.seen()
    );
    // The recording says which kind of turn it was, so the next person
    // reading a flight log can tell a turn with no `stdin` line above it
    // from a bug.
    until("the recording to name the implicit turn", || {
        harness.recording().iter().any(|event| {
            matches!(
                event,
                crate::record::Event::Lifecycle {
                    event: crate::record::Stage::TurnOpen,
                    detail: Some(detail),
                    ..
                } if detail.contains("a turn of its own")
            )
        })
    })
    .await;
}

/// The other half of the same bug, and the reason it was worth fixing.
///
/// A message sent during a turn claude opened for itself used to be judged
/// as arriving between turns, so it went straight into stdin — where the CLI
/// folded it into the reply already running and it never became a `user`
/// entry at all. With the turn accounted for, the queue machinery that
/// already existed takes it: journalled on arrival, written when the turn
/// claude opened ends, and answered in a turn of its own.
#[tokio::test]
async fn a_message_sent_during_a_turn_claude_opened_waits_for_it() {
    let harness = Harness::ordinary_with(&wakes_itself_for_a_finished_task()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.nudge("subagent");
    harness.nudge("wake");
    harness.nudge("speak");
    until("the turn claude opened", || harness.state().phase == Phase::Working).await;

    harness.send("所以结论呢？");
    until("the message to be queued", || harness.state().queued == 1).await;
    assert!(
        harness.seen().is_empty(),
        "the message was not written into the middle of the turn: {:?}",
        harness.seen()
    );
    // On disk as well as in memory: a restart in this window has to find it.
    until("the journal to hold it", || {
        harness.record().payload.queued.iter().any(|queued| queued.text.contains("所以结论呢"))
    })
    .await;

    // The turn ends, and the queue behind it moves — into a turn of its own,
    // which is what gives the message a `user` entry and an answer.
    harness.nudge("finish");
    until("the message to reach claude in its own turn", || {
        harness.state().phase == Phase::Working && harness.seen().iter().any(|line| line.contains("所以结论呢"))
    })
    .await;
    harness.release();
    until("the queue to drain", || {
        harness.state().queued == 0 && harness.state().phase == Phase::Idle
    })
    .await;
}

/// A claude that writes the `allowed` event before every turn's result, as
/// the real one does.
fn reports_its_windows() -> String {
    format!(
        r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
*control_request*) : ;;
*) printf '%s\n' '{ALLOWED_WITH_WINDOWS}'
   printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}' ;;
  esac
done
"#
    )
}

/// The live path end to end: a session on the local login writes an
/// `allowed` event, and the panel's reading of the local login has it.
#[tokio::test]
async fn every_rate_limit_event_is_filed_under_the_login_it_ran_on() {
    let harness = Harness::ordinary_with(&reports_its_windows()).await;
    until("the init to land", || harness.state().phase == Phase::Idle).await;
    harness.send("first");
    let file = harness.registry.host.paths.data_dir().join("local-usage.json");
    until("the reading to be filed", || file.is_file()).await;

    let reading = harness.registry.host.quota.cached(&droppen_accounts::quota::UsageLogin::Local);
    let windows: Vec<(&str, f64)> = reading.windows.iter().map(|w| (w.label.as_str(), w.used_percent)).collect();
    assert_eq!(windows, [("5h", 19.0), ("weekly", 46.0)]);
    assert_eq!(reading.source, Some(droppen_accounts::quota::QuotaReadingSource::Session));
    assert!(reading.updated_at.is_some());
}

/// The whole cycle, twice, on one process — which is the entire point of
/// the change. Under the old model the second message would have been a
/// second `claude`.
#[tokio::test]
async fn a_turn_runs_from_init_to_result_and_the_process_lives_on() {
    let harness = Harness::ordinary().await;
    until("the init to land", || harness.state().phase == Phase::Idle).await;
    assert!(harness.injection().is_none(), "an idle resident has no run to report");

    harness.send("first");
    until("the turn to open", || harness.state().phase == Phase::Working).await;
    assert_eq!(
        harness.injection().expect("a run").state,
        InjectionRunState::Running,
        "a turn in flight is what the composer sees"
    );

    harness.release();
    until("the result", || harness.state().phase == Phase::Idle).await;
    let injection = harness.injection().expect("a run");
    assert_eq!(injection.state, InjectionRunState::Ok);
    assert!(injection.finished_at.0.is_some(), "and it has an end time");

    // The same process takes the next one.
    harness.send("second");
    until("the second turn", || harness.state().phase == Phase::Working).await;
    harness.release();
    until("the second result", || harness.state().phase == Phase::Idle).await;
    assert_eq!(harness.injection().expect("a run").state, InjectionRunState::Ok);

    let seen = harness.seen();
    assert_eq!(seen.len(), 2, "two messages, one process: {seen:?}");
    assert!(seen[0].contains("\"first\""), "{seen:?}");
    assert!(!harness.driver.is_finished(), "the process is still there for a third");
}

/// A turn's alert carries what its driver's snapshot says of the session:
/// its resident, idle as the turn left it ([`RunHost::driver_states_of`]).
#[tokio::test]
async fn a_turn_s_alert_sees_its_session_s_resident() {
    let harness = Harness::ordinary().await;
    let alerts = Arc::new(droppen_provider::testing::TurnAlerts::default());
    harness.registry.host.attach_alerts(alerts.clone());
    until("the init to land", || harness.state().phase == Phase::Idle).await;
    harness.send("first");
    until("the turn to open", || harness.state().phase == Phase::Working).await;
    harness.release();
    until("the turn's alert", || !alerts.seen().is_empty()).await;
    let (source, id, resident) = alerts.seen().remove(0);
    assert_eq!((source, id.as_str()), (Source::Claude, SESSION));
    assert_eq!(resident.map(|resident| resident.phase), Some(droppen_sessions::ResidentPhase::Idle), "its resident, as the turn left it");
}

#[tokio::test]
async fn uploads_directory_failure_prevents_spawning_a_misconfigured_process() {
    let harness = Harness::ordinary().await;
    let paths = &harness.registry.host.paths;
    std::fs::write(paths.uploads_dir(), b"not a directory").unwrap();
    let recipe = super::spawn::claude_recipe(paths, harness.bin.to_str(), &Default::default(), Env::new(), Default::default());
    let recorder = harness.registry.recorders.open(SESSION);
    let permit = harness.providers.gate().empty_lease().respawn_permit();
    let result = harness.registry.spawn_link(&permit, SESSION, &recipe, Selector::Fresh, None, &[], &recorder).await;
    assert!(matches!(result, Err(InjectError::Message(ref message)) if message.contains("cannot prepare the uploads directory")));
    assert!(harness.spawns().is_empty());
}

#[tokio::test]
async fn text_then_attachments_keep_the_upload_enabled_resident() {
    let harness = Harness::ordinary().await;
    until("the original init", || harness.state().phase == Phase::Idle).await;
    assert!(!harness.registry.host.paths.uploads_dir().exists());
    // A real spawn through the production recipe, leaving background work
    // alive between turns so a spurious settings change would park or kill it.
    let script = replacement_claude(SESSION, FORKED).replace(
        "*) printf",
        "*) printf '%s\\n' '{\"type\":\"system\",\"subtype\":\"task_started\",\"task_id\":\"uploads-task\",\"description\":\"still working\"}'; printf",
    );
    harness.plan_claude(&script);
    let make_recipe = |options: &droppen_provider::SendOptions| {
        super::spawn::claude_recipe(
            &harness.registry.host.paths,
            harness.bin.to_str(),
            options,
            harness.recipe(Fingerprint::default()).env,
            Default::default(),
        )
    };
    harness.send_with("text first", make_recipe(&Default::default()));
    until("the upload-enabled process to finish the first turn", || {
        harness.after().len() == 1 && harness.state().phase == Phase::Idle
    })
    .await;
    assert!(harness.registry.host.paths.uploads_dir().is_dir());
    assert!(harness.recorded_fingerprint().unwrap().uploads);
    let first = harness.registry.host.journal.read().into_iter().find(|r| r.session_id == SESSION).unwrap();
    assert_eq!(harness.spawns().len(), 1);
    assert!(harness.spawns()[0].contains("--add-dir"));
    for (i, options) in [
        droppen_provider::SendOptions {
            images: vec!["/uploads/screenshot.png".into()],
            ..Default::default()
        },
        droppen_provider::SendOptions {
            files: vec!["/uploads/log.txt".into()],
            ..Default::default()
        },
        droppen_provider::SendOptions::default(),
    ]
    .into_iter()
    .enumerate()
    {
        let text = super::spawn::with_claude_attachments("next message", &options);
        harness.send_with(&text, make_recipe(&options));
        until("the same process to receive the next message", || {
            harness.after().len() == i + 2 && harness.state().phase == Phase::Idle
        })
        .await;
        let current = harness.registry.host.journal.read().into_iter().find(|r| r.session_id == SESSION).unwrap();
        assert_eq!(current.cli_pid, first.cli_pid);
        assert_eq!(current.run_id, first.run_id);
        assert_eq!(harness.spawns().len(), 1);
        assert!(!harness.state().awaiting_background);
        assert_eq!(harness.state().background, 1);
    }
    assert!(harness.after()[1].contains("screenshot.png"));
    assert!(harness.after()[2].contains("log.txt"));
}

/// The behaviour of the real CLI that every other test's fake glosses
/// over: claude 2.1.231 given a session selector says *nothing* — not even
/// `init` — until it has read a first message. A driver that waits for
/// `init` to deliver deadlocks such a process for ever, which is exactly
/// what happened the first time this module met a real claude.
#[tokio::test]
async fn a_claude_that_says_nothing_until_spoken_to_still_gets_the_message() {
    let harness = Harness::start_with_handshake(
        "", // no init up front: the body speaks only once it has read
        &format!(
            r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'
  printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}'
done
"#
        ),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
    )
    .await;
    harness.send("wake up");
    until("the whole turn to run on a taciturn claude", || {
        harness.injection().is_some_and(|run| run.state == InjectionRunState::Ok)
    })
    .await;
    assert!(harness.seen()[0].contains("wake up"), "{:?}", harness.seen());
}

/// The queue is FIFO and one-at-a-time: a message sent mid-turn waits, and
/// nothing reaches claude until the turn before it has ended.
#[tokio::test]
async fn messages_sent_during_a_turn_are_delivered_in_order_afterwards() {
    let harness = Harness::ordinary().await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send("one");
    until("the first turn", || harness.state().phase == Phase::Working).await;
    harness.send("two");
    harness.send("three");
    until("both to be queued", || harness.state().queued == 2).await;
    assert_eq!(harness.seen().len(), 1, "nothing jumps the turn in flight");

    // One release per turn, each only once the turn before it has actually
    // been picked up: the fake claude holds exactly one turn at a time, and
    // releasing twice into the same one would prove nothing about order.
    harness.release();
    until("two to be read", || harness.seen().len() == 2).await;
    harness.release();
    until("three to be read", || harness.seen().len() == 3).await;
    harness.release();
    until("the queue to drain", || {
        harness.state().queued == 0 && harness.state().phase == Phase::Idle
    })
    .await;

    let seen = harness.seen();
    assert_eq!(seen.len(), 3);
    for (index, expected) in ["one", "two", "three"].iter().enumerate() {
        assert!(seen[index].contains(&format!("\"{expected}\"")), "out of order: {seen:?}");
    }
}

/// The message really is stream-json on stdin, in the shape claude parses.
#[tokio::test]
async fn a_message_reaches_claude_as_one_stream_json_user_line() {
    let harness = Harness::ordinary().await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("hello \"world\"");
    until("the message", || !harness.seen().is_empty()).await;

    let line = harness.seen().remove(0);
    let value: Value = serde_json::from_str(&line).expect("stdin carries one JSON object per line");
    assert_eq!(value["type"], "user");
    assert_eq!(value["message"]["role"], "user");
    assert_eq!(value["message"]["content"][0]["type"], "text");
    assert_eq!(value["message"]["content"][0]["text"], "hello \"world\"");
}

/// The line a message goes in as, whole, sent and steered alike: key
/// order is part of what claude is handed, and a parsed `Value` compares
/// equal in any order. Quotes, control characters and non-ASCII text are
/// where two ways of writing it could differ.
#[tokio::test]
async fn a_sent_and_a_steered_message_reach_claude_as_exactly_these_bytes() {
    let harness = Harness::ordinary().await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("hello \"world\"\nsecond line — 你好");
    until("the turn", || harness.state().phase == Phase::Working).await;
    harness.send("urgent\tnow");
    until("it to be queued", || harness.state().queued == 1).await;
    assert_eq!(harness.steer(&harness.qid("urgent")).await, QueueOutcome::Done);
    harness.release();
    until("the steered message to be read", || harness.seen().len() == 2).await;
    assert_eq!(
        harness.seen(),
        [
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello \"world\"\nsecond line — 你好"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"urgent\tnow"}]}}"#,
        ]
    );
}

// ----------------------------------------------------------- the queue

/// What a row can now say about a queue, which used to be a number.
#[tokio::test]
async fn every_queued_message_is_named_and_previewed() {
    let harness = Harness::ordinary().await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send("one");
    until("the first turn", || harness.state().phase == Phase::Working).await;
    assert!(harness.queued().is_empty(), "the message in flight is a turn, not a queue");

    harness.send(&"long ".repeat(100));
    harness.send("two");
    until("both to be queued", || harness.state().queued == 2).await;

    let queued = harness.queued();
    assert_eq!(queued.len(), 2, "{queued:?}");
    assert_eq!(queued[1].text, "two", "the queue is in the order it will run");
    assert_eq!(
        queued[0].text.chars().count(),
        QUEUE_PREVIEW_CAP + 1,
        "a long prompt is previewed, ellipsis included: {:?}",
        queued[0].text
    );
    assert!(queued[0].text.ends_with('…'));
    assert!(queued.iter().all(|message| !message.steered), "nothing was steered: {queued:?}");
    assert_ne!(queued[0].id, queued[1].id, "two messages are two ids");

    // The ids outlive the wait, and disappear with the message rather than
    // being handed on to whatever takes its place.
    let first = queued[0].id.clone();
    harness.release();
    until("the long one to start", || harness.state().queued == 1).await;
    assert!(
        harness.queued().iter().all(|message| message.id != first),
        "a message that has started is no longer in the queue"
    );
}

/// The point of the whole mechanism: a correction that cannot wait goes into
/// claude's stdin now, and therefore runs before messages sent before it.
///
/// The fake claude models the CLI behaviour this rests on — it reads its
/// stdin one line at a time, so a line written mid-turn sits in the pipe
/// until the turn it interrupted has finished. That is exactly what claude
/// 2.1.231 does with a `user` line written under stream-json.
#[tokio::test]
async fn a_steered_message_runs_before_the_queue_it_jumped() {
    let harness = Harness::ordinary().await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send("one");
    until("the turn", || harness.state().phase == Phase::Working).await;
    harness.send("two");
    harness.send("urgent");
    until("both to be queued", || harness.state().queued == 2).await;

    let urgent = harness.qid("urgent");
    assert_eq!(harness.steer(&urgent).await, QueueOutcome::Done);

    // Still outstanding — it has not run — but no longer withdrawable, and
    // the row says which of the two it is.
    assert_eq!(harness.state().queued, 2, "a steered message is still waiting to run");
    let queued = harness.queued();
    assert_eq!(queued[0].id, urgent, "it runs first now: {queued:?}");
    assert!(queued[0].steered, "{queued:?}");
    assert!(!queued[1].steered, "and the message it jumped is untouched: {queued:?}");
    assert_eq!(harness.seen().len(), 1, "the turn in flight was not interrupted or re-prompted");

    // The turn in flight ends, and claude answers what is in its stdin
    // before anything this driver has left to write.
    harness.release();
    until("the steered message to be read", || harness.seen().len() == 2).await;
    assert!(harness.seen()[1].contains("\"urgent\""), "{:?}", harness.seen());
    // Both halves, because either alone is ambiguous: the process reads the
    // steered line as soon as its own turn ends, which is concurrent with
    // this driver noticing the `result`, so `Working` on its own can still
    // be the *previous* turn. `Working` with one message left is only ever
    // the steered one running.
    until("the steered message's turn to open", || {
        let state = harness.state();
        state.phase == Phase::Working && state.queued == 1
    })
    .await;
    let queued = harness.queued();
    assert_eq!(queued.len(), 1, "the steered message is running, not queued: {queued:?}");
    assert_eq!(queued[0].text, "two", "what is left is the message it jumped");

    harness.release();
    until("the message it jumped to be read", || harness.seen().len() == 3).await;
    assert!(harness.seen()[2].contains("\"two\""), "{:?}", harness.seen());
    harness.release();
    until("the queue to drain", || {
        harness.state().queued == 0 && harness.state().phase == Phase::Idle
    })
    .await;
    assert_eq!(
        harness.injection().expect("a run").state,
        InjectionRunState::Ok,
        "every turn ended normally"
    );
    assert!(staged(&harness.recording(), crate::record::Stage::Steer), "the recording says why");
}

/// The session-killer, reproduced: claude answers the steered message
/// *inside* the turn it was written into, and says nothing more.
///
/// What used to happen here is the whole reason this test exists. The
/// driver opened a turn for the steered message on the strength of its own
/// bookkeeping, no `result` for it ever came — the CLI had already answered
/// it — and the session showed `working` for ever with the ordinary queue
/// pinned behind a turn nothing could end. Only Stop got out of it.
///
/// Now the turn is provisional, the silence is the answer, and the queue
/// moves on its own.
#[tokio::test]
async fn a_steered_message_the_turn_swallowed_does_not_wedge_the_queue() {
    let settle = Duration::from_millis(200);
    let harness = Harness::settling(folds_the_steered_message(), settle, Duration::from_millis(120)).await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send("slow");
    until("the turn", || harness.state().phase == Phase::Working).await;
    harness.send("after");
    harness.send("urgent");
    until("both to be queued", || harness.state().queued == 2).await;

    let urgent = harness.qid("urgent");
    assert_eq!(harness.steer(&urgent).await, QueueOutcome::Done);
    // The fake claude has to have *read* the steered line before the turn is
    // released, or it would answer it separately and this would be the other
    // scenario. That is exactly what the real CLI's injection window is.
    until("claude to take the steered message into the running turn", || harness.seen().len() == 2).await;

    harness.release();

    // One `result`, covering both prompts. The queue behind it must move
    // anyway, on its own: nothing else in this harness would move it (no
    // stall, no tick), so `until`'s ten seconds are the bound, and the
    // recording below says it was the settle window that moved it. A
    // bound in wall-clock seconds measured the machine as much as the
    // driver.
    until("the message it jumped to start on its own", || {
        harness.seen().iter().any(|line| line.contains("\"after\""))
    })
    .await;
    let state = harness.state();
    assert_eq!(state.phase, Phase::Working, "the message it jumped is running: {state:?}");
    assert_eq!(state.queued, 0, "and nothing is left waiting: {state:?}");

    harness.release();
    until("the session to go quiet", || {
        harness.state().queued == 0 && harness.state().phase == Phase::Idle
    })
    .await;
    assert_eq!(
        harness.injection().expect("a run").state,
        InjectionRunState::Ok,
        "no turn was left running and none was failed"
    );
    assert_eq!(harness.seen().len(), 3, "and nothing was written twice: {:?}", harness.seen());
    assert!(
        staged(&harness.recording(), crate::record::Stage::Absorb),
        "the recording says which of the two things the CLI did: {:?}",
        harness.recording()
    );

    // The ghost, stated as the assertion it is: `Working` outlives nothing.
    tokio::time::sleep(settle * 3).await;
    assert_eq!(harness.state().phase, Phase::Idle, "no turn is left running");
}

/// The other thing the same CLI does: the steered message is buffered and
/// answered as a turn of its own, `system:init` and all.
///
/// The half that must not regress while the half above is fixed, and the
/// settle window is deliberately short here to prove it cannot: the
/// deferred turn announces itself and is then held for many times the
/// window, so a driver that judged on silence alone would close it early —
/// and the `result` that eventually arrived would land on the *next*
/// message's turn, failing it for something it never did.
#[tokio::test]
async fn a_steered_message_claude_answers_separately_keeps_its_own_turn() {
    let settle = Duration::from_millis(120);
    let harness = Harness::settling(&defers_the_steered_message(), settle, Duration::from_millis(120)).await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send("slow");
    until("the turn", || harness.state().phase == Phase::Working).await;
    harness.send("after");
    harness.send("urgent");
    until("both to be queued", || harness.state().queued == 2).await;

    let urgent = harness.qid("urgent");
    assert_eq!(harness.steer(&urgent).await, QueueOutcome::Done);
    harness.release();

    // claude reads the steered line, announces a turn for it, and holds it.
    until("the steered message to be read", || harness.seen().len() == 2).await;
    assert!(harness.seen()[1].contains("\"urgent\""), "{:?}", harness.seen());
    until("its own turn to be running", || {
        let state = harness.state();
        state.phase == Phase::Working && state.queued == 1
    })
    .await;

    // Far longer than the settle window, with nothing on stdout: the `init`
    // is what holds the turn open, and it has to be enough on its own.
    tokio::time::sleep(settle * 5).await;
    let state = harness.state();
    assert_eq!(state.phase, Phase::Working, "the deferred turn is still running: {state:?}");
    assert_eq!(state.queued, 1, "and the message behind it has not been let out: {state:?}");
    assert_eq!(harness.seen().len(), 2, "nothing jumped it: {:?}", harness.seen());

    // Its `result` ends its own turn and nobody else's, and what it was
    // holding up starts next.
    harness.release();
    until("the message it jumped to run", || harness.seen().len() == 3).await;
    assert!(harness.seen()[2].contains("\"after\""), "{:?}", harness.seen());
    harness.release();
    until("the queue to drain", || {
        harness.state().queued == 0 && harness.state().phase == Phase::Idle
    })
    .await;
    assert_eq!(
        harness.injection().expect("a run").state,
        InjectionRunState::Ok,
        "the message the steered one jumped ran to a normal end"
    );
}

/// The CLI as it is now: a steered message **folded** into the turn that is
/// running, with the transcript the only thing that says so.
///
/// The turn's opening is on file before anything is steered — the prompt,
/// a tool call, and an *older* fold of the same words from earlier in the
/// session, which must not be mistaken for this one. Once the steered line
/// has been read and the test says `fold`, the CLI's record of the fold is
/// appended the way a real session wrote it (`3c002577`): `enqueue`, the
/// `queued_command` attachment, `remove`. Then the turn keeps running until
/// released — the stretch the queue used to be wrong for.
fn folds_into_the_transcript() -> &'static str {
    r#"
IFS= read -r first
printf '%s\n' "$first" >> "$LOG"
printf '%s\n' '{"type":"attachment","attachment":{"type":"queued_command","prompt":"urgent","commandMode":"prompt"},"timestamp":"2026-10-18T09:00:00.000Z"}' >> "$TRANSCRIPT"
printf '%s\n' '{"type":"user","message":{"role":"user","content":[{"type":"text","text":"slow"}]}}' >> "$TRANSCRIPT"
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}}]}}' >> "$TRANSCRIPT"
touch "$GO_DIR/opened"
IFS= read -r second
printf '%s\n' "$second" >> "$LOG"
await_file fold
printf '%s\n' '{"type":"queue-operation","operation":"enqueue","content":"urgent"}' >> "$TRANSCRIPT"
printf '%s\n' '{"type":"attachment","attachment":{"type":"queued_command","prompt":[{"text":"urgent","type":"text"}],"commandMode":"prompt"}}' >> "$TRANSCRIPT"
printf '%s\n' '{"type":"queue-operation","operation":"remove"}' >> "$TRANSCRIPT"
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"answering both"}]}}' >> "$TRANSCRIPT"
touch "$GO_DIR/folded"
await_go
rm -f "$GO"
printf '%s\n' '{"type":"assistant","message":{}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
*control_request*) : ;;
*) await_go
   rm -f "$GO"
   printf '%s\n' '{"type":"result","subtype":"success","is_error":false}' ;;
  esac
done
"#
}

/// The same CLI **buffering** the steered message for a turn of its own,
/// and writing down what it really writes for that: an `enqueue` with the
/// words while the turn runs, then `dequeue` and an ordinary `user` entry
/// once its own turn — announced by `init` — begins.
fn buffers_into_the_transcript() -> String {
    format!(
        r#"
IFS= read -r first
printf '%s\n' "$first" >> "$LOG"
printf '%s\n' '{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"slow"}}]}}}}' >> "$TRANSCRIPT"
printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"t1","name":"Bash","input":{{}}}}]}}}}' >> "$TRANSCRIPT"
IFS= read -r second
printf '%s\n' "$second" >> "$LOG"
printf '%s\n' '{{"type":"queue-operation","operation":"enqueue","content":"urgent"}}' >> "$TRANSCRIPT"
touch "$GO_DIR/buffered"
await_go
rm -f "$GO"
printf '%s\n' '{{"type":"assistant","message":{{}}}}'
printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}'
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'
printf '%s\n' '{{"type":"queue-operation","operation":"dequeue"}}' >> "$TRANSCRIPT"
printf '%s\n' '{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"urgent"}}]}}}}' >> "$TRANSCRIPT"
await_go
rm -f "$GO"
printf '%s\n' '{{"type":"assistant","message":{{}}}}'
printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
*control_request*) : ;;
*) await_go
   rm -f "$GO"
   printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}' ;;
  esac
done
"#
    )
}

/// Whether the fake claude has touched `step` in the harness' directory.
fn reached(harness: &Harness, step: &str) -> bool {
    harness.go.parent().expect("the go file has a directory").join(step).exists()
}

/// The bug, as a user saw it: a message steered into a running turn was
/// folded into that turn and answered there, while the queue went on saying
/// "delivered, waiting to start" — the row, the count, the journal — until
/// the turn ended, with a Stop hint beside it that no longer meant anything.
///
/// Now the transcript's record of the fold takes it off the queue while the
/// turn is still running. Two neighbours that must not move with it: an
/// identical message still waiting in the queue proper (never delivered, so
/// no fold can be its), and an older fold of the same words that was in the
/// transcript before the steer. And the turn's end must open no turn for the
/// folded message — with a settle nobody can wait out, a provisional one
/// would hold everything behind it.
#[tokio::test]
async fn a_steer_folded_into_the_running_turn_leaves_the_queue_while_that_turn_runs() {
    let harness = Harness::start(
        folds_into_the_transcript(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
    )
    .await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send("slow");
    until("the turn to be on file", || reached(&harness, "opened")).await;
    harness.send("urgent");
    harness.send("urgent");
    until("both to be queued", || harness.state().queued == 2).await;
    let ids: Vec<String> = harness.queued().into_iter().map(|message| message.id).collect();
    let (waiting, steered) = (ids[0].clone(), ids[1].clone());
    assert_eq!(harness.steer(&steered).await, QueueOutcome::Done);
    until("claude to read the steered line", || harness.seen().len() == 2).await;

    // Read, not yet folded: claude's, and still to come. Six polls go by,
    // and the fold already in the history is not mistaken for this one.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let queued = harness.queued();
    assert_eq!(queued.len(), 2, "{queued:?}");
    assert!(queued[0].id == steered && queued[0].steered, "{queued:?}");

    harness.nudge("fold");
    until("the fold to take it off the queue", || harness.state().queued == 1).await;
    let state = harness.state();
    assert_eq!(state.phase, Phase::Working, "the turn it was folded into is still running: {state:?}");
    assert_eq!(state.queued_messages.len(), 1, "{state:?}");
    assert_eq!(
        state.queued_messages[0].id, waiting,
        "the identical message in the queue is untouched: {state:?}"
    );
    assert!(!state.queued_messages[0].steered, "and still undelivered: {state:?}");
    let journalled: Vec<_> = harness.record().payload.queued.into_iter().map(|queued| queued.id).collect();
    assert_eq!(
        journalled,
        std::slice::from_ref(&waiting),
        "the journal no longer holds the folded one either"
    );

    // One `result`, covering both prompts; the waiting message is next.
    harness.release();
    until("the waiting message to go in", || harness.seen().len() == 3).await;
    assert!(harness.seen()[2].contains("\"urgent\""), "{:?}", harness.seen());
    assert_eq!(harness.state().queued, 0, "{:?}", harness.state());
    harness.release();
    until("the session to go quiet", || harness.state().phase == Phase::Idle).await;
    assert_eq!(harness.seen().len(), 3, "nothing was written twice: {:?}", harness.seen());
    assert_eq!(harness.injection().expect("a run").state, InjectionRunState::Ok);
    assert!(
        staged(&harness.recording(), crate::record::Stage::Absorb),
        "the recording says the message was folded: {:?}",
        harness.recording()
    );
}

/// A watched session does not wait for the poll: its feed, having just sent
/// the grown transcript, tells the resident to look — so the message leaves
/// the queue on screen in the moment it appears in the transcript.
#[tokio::test]
async fn the_feed_seeing_the_fold_is_enough_on_its_own() {
    let harness = Harness::build(
        &preamble(),
        folds_into_the_transcript(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
        NEVER_SETTLES,
        Adoption {
            absorb_poll: NEVER_SETTLES,
            ..Adoption::default()
        },
    )
    .await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("slow");
    until("the turn to be on file", || reached(&harness, "opened")).await;
    harness.send("urgent");
    until("it to be queued", || harness.state().queued == 1).await;
    assert_eq!(harness.steer(&harness.qid("urgent")).await, QueueOutcome::Done);
    harness.nudge("fold");
    until("the fold to be on file", || reached(&harness, "folded")).await;

    // No poll is coming, so nothing has looked yet.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(harness.state().queued, 1, "{:?}", harness.state());

    harness.providers().transcript_grew(Source::Claude, SESSION);
    until("the nudge to take it off the queue", || harness.state().queued == 0).await;
    assert_eq!(harness.state().phase, Phase::Working, "the turn is still running");
    // A nudge for a source with no residents of this kind changes nothing.
    harness.providers().transcript_grew(Source::Codex, SESSION);

    harness.release();
    until("the session to go quiet", || harness.state().phase == Phase::Idle).await;
    assert_eq!(harness.seen().len(), 2, "{:?}", harness.seen());
}

/// Neither poll nor nudge has come round when the turn ends — the fold was
/// the last thing before the `result`. The turn's end reads for it once
/// more before opening a turn for the steered message, so no provisional
/// turn is opened for something already answered.
#[tokio::test]
async fn the_turns_end_catches_a_fold_nothing_else_saw() {
    let harness = Harness::build(
        &preamble(),
        folds_into_the_transcript(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
        NEVER_SETTLES,
        Adoption {
            absorb_poll: NEVER_SETTLES,
            ..Adoption::default()
        },
    )
    .await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("slow");
    until("the turn to be on file", || reached(&harness, "opened")).await;
    harness.send("urgent");
    until("it to be queued", || harness.state().queued == 1).await;
    assert_eq!(harness.steer(&harness.qid("urgent")).await, QueueOutcome::Done);
    harness.nudge("fold");
    until("the fold to be on file", || reached(&harness, "folded")).await;
    assert_eq!(harness.state().queued, 1, "nothing has looked yet");

    harness.release();
    // With a settle nobody can wait out, a provisional turn would leave the
    // session `working` for ever.
    until("the session to go quiet", || {
        let state = harness.state();
        state.phase == Phase::Idle && state.queued == 0
    })
    .await;
    assert_eq!(harness.seen().len(), 2, "{:?}", harness.seen());
    assert_eq!(harness.injection().expect("a run").state, InjectionRunState::Ok);
}

/// A steered message the CLI holds for a turn of its own is *not* folded,
/// whatever the transcript says about it in the meantime: an `enqueue` with
/// its words while the turn runs, then a `user` entry once its own turn
/// starts. It stays queued — delivered, waiting to start — for the whole of
/// the running turn, and gets that turn when it comes.
#[tokio::test]
async fn a_steer_held_for_its_own_turn_stays_queued_until_that_turn() {
    let settle = Duration::from_millis(120);
    let harness = Harness::settling(&buffers_into_the_transcript(), settle, Duration::from_millis(120)).await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("slow");
    until("the turn", || harness.state().phase == Phase::Working).await;
    harness.send("after");
    harness.send("urgent");
    until("both to be queued", || harness.state().queued == 2).await;
    assert_eq!(harness.steer(&harness.qid("urgent")).await, QueueOutcome::Done);
    until("claude to buffer it", || reached(&harness, "buffered")).await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    let queued = harness.queued();
    assert_eq!(queued.len(), 2, "still waiting for a turn of its own: {queued:?}");
    assert!(queued[0].text == "urgent" && queued[0].steered, "{queued:?}");
    assert!(queued[1].text == "after" && !queued[1].steered, "{queued:?}");

    harness.release();
    until("its own turn to be running", || {
        let state = harness.state();
        state.phase == Phase::Working && state.queued == 1
    })
    .await;
    // Many times the settle window: `init` made the turn real, and the
    // `user` entry the transcript gained is not a fold.
    tokio::time::sleep(settle * 5).await;
    let state = harness.state();
    assert_eq!(state.phase, Phase::Working, "{state:?}");
    assert_eq!(state.queued_messages.len(), 1, "{state:?}");
    assert_eq!(state.queued_messages[0].text, "after", "{state:?}");
    assert_eq!(harness.seen().len(), 2, "nothing jumped it: {:?}", harness.seen());

    harness.release();
    until("the message it jumped to run", || harness.seen().len() == 3).await;
    harness.release();
    until("the queue to drain", || {
        harness.state().queued == 0 && harness.state().phase == Phase::Idle
    })
    .await;
}

/// Stop, pressed in the window where nobody yet knows whether the steered
/// message has a turn of its own.
///
/// Nothing about the ladder changes — the polite request goes out, the
/// rungs are armed — but the two clocks now have to be read together. The
/// settle is the shorter of them (two seconds against the ladder's five,
/// and in the same ratio here), so a turn claude had already answered is
/// closed by the settle *first*, and the rung that comes due afterwards
/// finds no turn of that sequence and signals nothing. A session that had
/// finished its work is not killed for having been asked to stop it.
#[tokio::test]
async fn stop_inside_the_settle_window_neither_panics_nor_signals() {
    let settle = Duration::from_millis(100);
    let step = Duration::from_millis(800);
    let harness = Harness::settling(folds_the_steered_message(), settle, step).await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send("slow");
    until("the turn", || harness.state().phase == Phase::Working).await;
    harness.send("urgent");
    until("it to be queued", || harness.state().queued == 1).await;
    let urgent = harness.qid("urgent");
    assert_eq!(harness.steer(&urgent).await, QueueOutcome::Done);
    until("claude to take it into the running turn", || harness.seen().len() == 2).await;

    harness.release();
    until("the provisional turn", || {
        harness.state().queued == 0 && harness.state().phase == Phase::Working
    })
    .await;
    assert!(harness.registry.interrupt_resident(SESSION), "there is a turn to stop");

    until("the session to end up idle", || harness.state().phase == Phase::Idle).await;
    // Long enough for every rung of the ladder to have come due.
    tokio::time::sleep(step * 4).await;
    assert_eq!(harness.state().phase, Phase::Idle, "nothing re-opened the turn");
    assert!(
        harness.injection().is_some_and(|run| run.state != InjectionRunState::Running),
        "and no run was left in flight: {:?}",
        harness.injection()
    );
    assert!(
        !staged(&harness.recording(), crate::record::Stage::Escalate),
        "a turn claude had already answered was not signalled: {:?}",
        harness.recording()
    );
    assert!(
        !staged(&harness.recording(), crate::record::Stage::Exit),
        "and the process outlived the stop: {:?}",
        harness.recording()
    );
}

/// A message that needs a *different process* cannot jump the queue while a
/// turn is running: there is no stdin to write it to that would mean what
/// the caller asked for, and the restart it really needs would kill the turn
/// the user is watching.
///
/// The other half of the rule is
/// [`a_steer_from_idle_restarts_for_the_settings_it_asks_for`]: from idle
/// the same press *is* the restart.
#[tokio::test]
async fn a_message_that_needs_new_settings_cannot_be_steered_into_a_running_turn() {
    let harness = Harness::ordinary().await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send("one");
    until("the turn", || harness.state().phase == Phase::Working).await;
    harness.send_with("under a different model", harness.recipe(a_settings_change()));
    until("it to be queued", || harness.state().queued == 1).await;

    let qid = harness.qid("different model");
    let outcome = harness.steer(&qid).await;
    let QueueOutcome::NeedsRestart { differences } = &outcome else {
        panic!("a turn is in flight, so the process cannot be replaced under it: {outcome:?}");
    };
    assert!(
        differences.contains("opus"),
        "and the refusal names the setting in the way, because the route has nothing else to show: {differences}"
    );
    let queued = harness.queued();
    assert_eq!(queued.len(), 1, "a refused steer leaves the queue exactly as it was");
    assert!(!queued[0].steered, "{queued:?}");
    assert_eq!(harness.seen().len(), 1, "and nothing was written to the process it does not fit");
    assert!(harness.spawns().is_empty(), "nor was anything restarted under the turn");

    // Unknown ids are refused too, and are the ordinary case rather than a
    // typo: a message whose turn started while the request was in flight.
    assert_eq!(harness.steer("no-such-message").await, QueueOutcome::Unknown);
    assert_eq!(harness.withdraw("no-such-message").await, QueueOutcome::Unknown);
}

/// A queued message can be taken back, and a steered one cannot: the second
/// is in claude's stdin, and nothing here can reach into another process'
/// buffer.
#[tokio::test]
async fn a_queued_message_can_be_withdrawn_until_it_is_steered() {
    let harness = Harness::ordinary().await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send("one");
    until("the turn", || harness.state().phase == Phase::Working).await;
    harness.send("regretted");
    harness.send("urgent");
    until("both to be queued", || harness.state().queued == 2).await;

    let regretted = harness.qid("regretted");
    let urgent = harness.qid("urgent");
    assert_eq!(harness.steer(&urgent).await, QueueOutcome::Done);
    assert_eq!(
        harness.withdraw(&urgent).await,
        QueueOutcome::AlreadySteered,
        "claude has it; there is no taking it back"
    );
    assert_eq!(
        harness.steer(&urgent).await,
        QueueOutcome::AlreadySteered,
        "and steering it twice would write it twice"
    );

    assert_eq!(harness.withdraw(&regretted).await, QueueOutcome::Done);
    assert_eq!(harness.state().queued, 1, "only the steered one is left");
    assert_eq!(harness.withdraw(&regretted).await, QueueOutcome::Unknown, "and it is gone for good");

    harness.release();
    until("the steered message to run", || harness.seen().len() == 2).await;
    harness.release();
    until("the session to go quiet", || {
        harness.state().queued == 0 && harness.state().phase == Phase::Idle
    })
    .await;
    let seen = harness.seen();
    assert_eq!(seen.len(), 2, "the withdrawn message never reached claude: {seen:?}");
    assert!(!seen.iter().any(|line| line.contains("regretted")), "{seen:?}");
}

/// A steer writes into a pipe, and a pipe whose reader dies was never read.
///
/// The same rule the queue follows and the turn in flight does not: a
/// message claude never saw is delivered to its replacement, and a turn that
/// half-ran is failed rather than repeated.
#[tokio::test]
async fn a_steered_message_is_delivered_again_when_the_process_dies_holding_it() {
    let harness = Harness::ordinary_with(die_when_released()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send("doomed");
    until("the turn", || harness.state().phase == Phase::Working).await;
    harness.send("urgent");
    until("the queue", || harness.state().queued == 1).await;
    let urgent = harness.qid("urgent");
    assert_eq!(harness.steer(&urgent).await, QueueOutcome::Done);
    assert!(harness.queued()[0].steered);

    // The process dies with the steered line unread in its stdin.
    harness.release();
    until("the steered message to reach the replacement", || {
        harness.after().iter().any(|line| line.contains("\"urgent\""))
    })
    .await;
    until("its turn to finish", || {
        harness.state().queued == 0 && harness.injection().is_some_and(|run| run.state == InjectionRunState::Ok)
    })
    .await;
    assert!(
        !harness.after().iter().any(|line| line.contains("\"doomed\"")),
        "the turn that was in flight is still not retried: {:?}",
        harness.after()
    );
}

// ---------------------------------------------------------- the watchdog

/// The lesson this whole watchdog rewrite exists for: a quiet turn is
/// *reported*, never ended. A long tool call is a legitimate thing to be
/// doing, and killing it destroys work nobody can get back.
#[tokio::test]
async fn a_silent_turn_is_marked_stalled_and_left_running() {
    let harness = Harness::start(
        hold_until_released(),
        Duration::from_millis(60),
        Duration::from_millis(30),
        Duration::from_secs(3600),
    )
    .await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("a long one");
    until("the turn", || harness.state().phase == Phase::Working).await;
    until("the stall mark", || harness.state().stalled).await;

    // Nothing was ended: the turn is still in flight, and finishing it
    // normally still works.
    assert_eq!(harness.state().phase, Phase::Working, "a stalled turn is still a turn");
    assert_eq!(harness.injection().expect("a run").state, InjectionRunState::Running);
    harness.release();
    until("the result", || harness.state().phase == Phase::Idle).await;
    assert_eq!(harness.injection().expect("a run").state, InjectionRunState::Ok);
    assert!(!harness.state().stalled, "and the mark is gone with the turn");
}

/// The turn's clock, which a list row reads as "working · 3m": set when a
/// turn begins, held while it runs, and gone when it ends.
#[tokio::test]
async fn a_turn_carries_its_start_time_until_it_ends() {
    let harness = Harness::start(
        hold_until_released(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    )
    .await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    assert_eq!(harness.state().turn_started_at, None, "between turns there is no clock");
    let before = iso8601(SystemTime::now());
    harness.send("a long one");
    until("the turn", || harness.state().phase == Phase::Working).await;
    let started = harness.state().turn_started_at.expect("a working turn has a start");
    assert!(started >= before, "started when the turn did: {started} vs {before}");
    let info: droppen_sessions::ResidentInfo = harness.state().into();
    assert_eq!(info.turn_started_at.0.as_deref(), Some(started.as_str()), "and it is on the wire");
    harness.release();
    until("the result", || harness.state().phase == Phase::Idle).await;
    assert_eq!(harness.state().turn_started_at, None, "and the clock stops with the turn");
}

/// An idle resident is not on any clock at all. Under the old watchdog it
/// was: a process between messages made no progress by definition, so the
/// twenty-minute rule would have shot every one of them.
#[tokio::test]
async fn an_idle_resident_is_never_marked_however_long_it_waits() {
    let harness = Harness::start(
        hold_until_released(),
        Duration::from_millis(20),
        Duration::from_millis(20),
        Duration::from_secs(3600),
    )
    .await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!harness.state().stalled, "an idle process is not a stuck one");
    assert!(!harness.driver.is_finished(), "and it is emphatically still alive");
    // And it still answers.
    harness.send("still here?");
    until("the turn", || harness.state().phase == Phase::Working).await;
    harness.release();
    until("the result", || harness.state().phase == Phase::Idle).await;
}
