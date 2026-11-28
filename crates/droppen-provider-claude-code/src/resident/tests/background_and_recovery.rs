//! Background tasks, interrupt/recovery ladder, and fingerprint tests.

use super::*;

// ------------------------------------------------- background tasks

/// The history a re-attached driver reads: a task that started and never
/// finished, and a turn that ended after it.
///
/// A real shim's ring is finite. A `task_notification` written while no
/// server was attached, or lapped out of the buffer, or belonging to a
/// process that died mid-task, leaves exactly this shape behind — and there
/// is no later event that can ever remove the task, because the only one
/// that removes it is the one that went missing.
pub(super) fn history_with_an_orphan_task() -> String {
    [
        r#"printf '%s\n' '{"type":"system","subtype":"hook_started"}'"#.to_string(),
        format!(r#"printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'"#),
        r#"printf '%s\n' '{"type":"system","subtype":"task_started","task_id":"ghost","description":"a job nobody will ever hear the end of","task_type":"local_agent"}'"#.to_string(),
        r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false}'"#.to_string(),
    ]
    .join("\n")
}

/// The history that made ghost turns: an idle session, then one `system`
/// line written long after the turn it followed had ended.
///
/// Copied from the flight recording of session `83193b4a` (2026-09-07): the
/// last `result` at 06:21:45, then nothing for fifteen minutes, then
/// `commands_changed` — a hook had edited the command list — and then the
/// server restarted.
fn history_ending_in_a_system_line() -> String {
    [
        format!(r#"printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'"#),
        r#"printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}'"#.to_string(),
        r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false}'"#.to_string(),
        r#"printf '%s\n' '{"type":"system","subtype":"commands_changed","commands":[{"name":"rh-notify"}]}'"#.to_string(),
    ]
    .join("\n")
}

/// The bug this reconstruction rule exists for: a restart must not invent a
/// turn out of a line that is not work.
///
/// Session `83193b4a`, 2026-09-07. The turn ended at 06:21:45; at 06:36:35
/// claude published `commands_changed` while the session sat idle; at
/// 06:52:16 the server restarted and re-attached. The replay read that last
/// line as work, decided the history ended mid-turn, and opened a turn for
/// it — `running` with nothing in it for fifty-five minutes, one queued
/// message that could never be written, and a turn mark saved back to
/// `runs.json` so the *next* restart reconstructed the same ghost. Only a
/// hand-sent interrupt cleared it.
#[tokio::test]
async fn a_system_line_after_the_last_result_is_not_a_turn_to_inherit() {
    let harness = Harness::start_attached(
        &history_ending_in_a_system_line(),
        hold_until_released(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
        Witness::None,
    )
    .await;
    until("the replay to settle", || harness.state().phase == Phase::Idle).await;
    assert!(
        harness.injection().is_none(),
        "a session that is between turns has no run: {:?}",
        harness.injection()
    );
    assert_eq!(harness.record().payload.turn_at, None, "and nothing for the next boot to re-read as a turn");

    // The half that hurt: a message sent afterwards must go in rather than
    // park behind a turn that can never end.
    harness.send("are you there");
    until("the message to reach claude", || harness.seen().len() == 1).await;
}

/// The net under the reconstruction: whatever opened a turn, one claude's
/// own transcript says is over does not get to hold the queue for ever.
///
/// The rule needs both halves, and the test gives both: nothing at all on
/// stdout for the whole stall window, *and* a transcript that ends on the
/// assistant's answer and has not been touched since. A real turn fails the
/// first half — a long tool call still writes tool results — and a turn
/// waiting inside one fails the second, because the transcript ends on the
/// `tool_use` that has not come back.
#[tokio::test]
async fn a_turn_the_transcript_says_ended_is_let_go_of() {
    let harness = Harness::start(
        hold_until_released(),
        Duration::from_millis(60),
        Duration::from_millis(30),
        Duration::from_secs(3600),
    )
    .await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("a message this fake claude will never answer");
    until("the turn", || harness.state().phase == Phase::Working).await;

    // What claude would have left behind had this turn ended before the
    // attach: an answer, written long enough ago to be settled.
    let transcript = harness.transcript();
    std::fs::write(
        &transcript,
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"done\"}]}}\n",
    )
    .expect("transcript");
    long_ago(&transcript);

    until("the turn to be let go of", || harness.state().phase == Phase::Idle).await;
    assert!(harness.injection().is_none(), "the run is withdrawn, not failed: nothing ran");
    assert!(!harness.state().stalled, "and the session is not reported as stuck either");
}

/// Backdate a file past [`TRANSCRIPT_SETTLES_AFTER`], as the shell tests do
/// with `touch -t`.
pub(super) fn long_ago(path: &Path) {
    let when = SystemTime::now() - Duration::from_secs(3600);
    let file = std::fs::File::options().write(true).open(path).expect("open to backdate");
    file.set_times(std::fs::FileTimes::new().set_accessed(when).set_modified(when))
        .expect("backdate");
}

/// The four background lines claude 2.1.231 really writes, in order.
///
/// Every field is copied from the real flight recording of session
/// `3c002577` (2026-08-30): a `task_started` with its `tool_use_id`,
/// `subagent_type` and `task_type`; a snapshot naming two tasks; a thinner
/// snapshot that has dropped one and describes the other with less; and the
/// `task_notification` that ends it. Stepped rather than streamed so the
/// mirror can be read between each one.
fn describes_its_background_work() -> String {
    format!(
        r#"
(
  await_file started
  printf '%s\n' '{{"type":"system","subtype":"task_started","task_id":"a8725c02","tool_use_id":"toolu_01PfPz9","description":"survey the repository","subagent_type":"general-purpose","task_type":"local_agent"}}'
  await_file snapshot
  printf '%s\n' '{{"type":"system","subtype":"background_tasks_changed","tasks":[{{"task_id":"a8725c02","task_type":"local_agent","description":"survey the repository"}},{{"task_id":"buli8h4uy","task_type":"local_bash","description":"Fetch PR 6-9 titles and bodies"}}]}}'
  await_file prune
  printf '%s\n' '{{"type":"system","subtype":"background_tasks_changed","tasks":[{{"task_id":"buli8h4uy","task_type":"local_bash"}}]}}'
  await_file notify
  printf '%s\n' '{{"type":"system","subtype":"task_notification","task_id":"buli8h4uy","status":"completed","output_file":"","summary":"Fetch PR 6-9 titles and bodies"}}'
) &
{}
"#,
        hold_until_released()
    )
}

/// The reported gap: the CLI describes every background task it starts, and
/// this server kept a count.
///
/// A session could say "2 background tasks" and nothing else — not what they
/// were, not when they began — while 1,530 lines naming them had gone past
/// on the same stream. Everything asserted here was on the wire all along.
#[tokio::test]
async fn background_tasks_carry_what_claude_said_about_them() {
    let harness = Harness::ordinary_with(&describes_its_background_work()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    // `task_started` is the richest line, and the one that names the task.
    harness.nudge("started");
    until("the task", || harness.state().background == 1).await;
    let started = harness.state().tasks;
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].id, "a8725c02");
    assert_eq!(started[0].description.as_ref().map(String::as_str), Some("survey the repository"));
    assert_eq!(started[0].task_type.as_ref().map(String::as_str), Some("local_agent"));
    assert_eq!(
        started[0].tool_use_id.as_ref().map(String::as_str),
        Some("toolu_01PfPz9"),
        "the join key onto the transcript, which only this line carries"
    );
    let first_seen = started[0].started_at.clone();
    assert!(
        first_seen.as_ref().is_some_and(|at| at.contains('T')),
        "first seen by this server's clock, in iso8601: {first_seen:?}"
    );

    // The snapshot adds the second one, and does not disturb the first.
    harness.nudge("snapshot");
    until("the second task", || harness.state().background == 2).await;
    let both = harness.state().tasks;
    assert_eq!(
        both.iter().map(|task| task.id.as_str()).collect::<Vec<_>>(),
        ["a8725c02", "buli8h4uy"],
        "oldest first, which is the one that has been running longest"
    );
    assert_eq!(both[0].started_at, first_seen, "a republished snapshot does not restart the clock");
    assert_eq!(both[1].description.as_ref().map(String::as_str), Some("Fetch PR 6-9 titles and bodies"));
    // The snapshot is the thinner line — it names this same task with no
    // `tool_use_id` at all — and the join key survives it. Without this the
    // key would live for the couple of seconds between `task_started` and
    // the first republished list, which is to say it would never be seen.
    assert_eq!(
        both[0].tool_use_id.as_ref().map(String::as_str),
        Some("toolu_01PfPz9"),
        "a thinner snapshot does not erase the join key the richer line carried"
    );
    assert_eq!(
        both[1].tool_use_id.as_ref(),
        None,
        "and a task only ever named by a snapshot simply has none: null, not invented"
    );

    // The mirror is the API's own answer, so the shape a client sees is
    // asserted here rather than assumed from the driver's.
    let info = droppen_sessions::ResidentInfo::from(harness.state());
    assert_eq!(info.background, 2, "the count stays authoritative");
    assert_eq!(info.tasks.len(), 2);
    assert_eq!(info.tasks[1].task_type.as_ref().map(String::as_str), Some("local_bash"));
    assert_eq!(
        info.tasks[0].tool_use_id.as_ref().map(String::as_str),
        Some("toolu_01PfPz9"),
        "and it reaches the client, which is the only place the join can happen"
    );

    // A snapshot is the whole live list: what it omits has ended. And what
    // it describes less richly than `task_started` did keeps the better
    // description rather than having it blanked.
    harness.nudge("prune");
    until("the snapshot to prune", || harness.state().background == 1).await;
    let pruned = harness.state().tasks;
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0].id, "buli8h4uy");
    assert_eq!(
        pruned[0].description.as_ref().map(String::as_str),
        Some("Fetch PR 6-9 titles and bodies"),
        "a thinner snapshot does not blank what the richer line said"
    );

    // And a notification is terminal.
    harness.nudge("notify");
    until("the notification", || harness.state().background == 0).await;
    assert!(harness.state().tasks.is_empty(), "the description goes with the task");
}

/// The request that stops one background task, whole, uuid aside.
#[tokio::test]
async fn a_stop_task_request_reaches_claude_as_exactly_these_bytes() {
    let harness = Harness::ordinary_with(&describes_its_background_work()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.nudge("started");
    until("the task", || harness.state().background == 1).await;
    assert_eq!(harness.registry.stop_resident_task(SESSION, "a8725c02"), StopTaskOutcome::Asked);
    until("the request to be read", || harness.seen().len() == 1).await;
    assert_eq!(
        masked_request_id(&harness.seen()[0], "stop-"),
        r#"{"type":"control_request","request_id":"stop-<uuid>","request":{"subtype":"stop_task","task_id":"a8725c02"}}"#
    );
}

/// A message that needs a different process from the one running.
pub(super) fn a_settings_change() -> Fingerprint {
    Fingerprint {
        model: Some("opus".into()),
        ..Default::default()
    }
}

/// The bug this settle exists for: an orphan task must not hold the queue.
///
/// Without it the replayed `task_started` is believed for ever,
/// `minding_background` is permanently true, and the first message asking
/// for a different model parks behind a task that does not exist. A parked
/// driver sits at `Phase::Idle`, so Stop refuses it too — the message can
/// be neither delivered, nor failed, nor cancelled, for the life of the
/// process.
#[tokio::test]
async fn an_orphan_task_from_the_replay_does_not_hold_the_queue_for_ever() {
    let harness = Harness::start_attached(
        &history_with_an_orphan_task(),
        hold_until_released(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
        Witness::None,
    )
    .await;
    // Nothing has written a subagent transcript, so nothing corroborates the
    // task and the replay's account of it is dropped.
    until("the replay to settle", || harness.state().phase == Phase::Idle).await;
    assert!(harness.transcript().exists(), "the probe had a session to look under");
    assert_eq!(harness.state().background, 0, "an uncorroborated task is not believed");
    // Waited for rather than sampled: a recording is written by a task of
    // its own, so it is behind the mirror by however long that task takes to
    // be scheduled — which on a loaded machine is longer than the mirror
    // update this test just saw.
    until("the recording to say so", || staged(&harness.recording(), crate::record::Stage::Settle)).await;

    // The proof it is not merely counted differently: a settings change goes
    // straight through instead of parking.
    harness.send_with("under a different model", harness.recipe(a_settings_change()));
    until("the replacement to take the message", || harness.after().len() == 1).await;
    assert!(!harness.state().awaiting_background, "nothing was ever parked");
}

/// The other direction, which is the reason the drop is conditional: real
/// background work at the moment of an attach has left a fresh subagent
/// transcript, and that task is believed and waited for.
///
/// And then the ceiling: a park with no way out is its own bug, because a
/// parked driver is `Idle` and Stop refuses it. The wait is bounded, loud,
/// and visible on the row for the whole of it.
#[tokio::test]
async fn a_corroborated_task_is_waited_for_but_not_for_ever() {
    let harness = Harness::start_attached(
        &history_with_an_orphan_task(),
        hold_until_released(),
        Duration::from_secs(3600),
        // The watchdog is what comes back to a parked driver and asks again,
        // so it has to tick inside the test.
        Duration::from_millis(50),
        Duration::from_millis(120),
        // The witness: a subagent transcript the process wrote as it
        // started the task, which is what real background work leaves.
        Witness::Fresh,
    )
    .await;
    until("the replay to settle", || harness.state().phase == Phase::Idle).await;
    assert_eq!(harness.state().background, 1, "fresh subagent output corroborates the task");
    // Reconstructed from the history, description and all: a task that
    // survives a restart is exactly the one a user most wants named.
    let replayed = harness.state().tasks;
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].id, "ghost");
    assert_eq!(
        replayed[0].description.as_ref().map(String::as_str),
        Some("a job nobody will ever hear the end of")
    );
    assert_eq!(replayed[0].task_type.as_ref().map(String::as_str), Some("local_agent"));

    harness.send_with("under a different model", harness.recipe(a_settings_change()));
    until("the settings change to park", || harness.state().awaiting_background).await;
    assert_eq!(harness.state().queued, 1, "the message is waiting, and the row says so");
    assert!(harness.after().is_empty(), "and nothing was restarted under it yet");

    // `await_background` in the test registry is 400ms, so the ceiling is
    // reached inside the test rather than in half an hour.
    until("the ceiling to release it", || !harness.after().is_empty()).await;
    assert!(!harness.state().awaiting_background, "the park ended when the ceiling did");
    assert_eq!(harness.state().background, 0, "and the replacement inherited no tasks");
}

/// The other way out of a park, and the one a user can actually reach: the
/// steer button.
///
/// The bug it fixes had no way out at all. A settings change parked behind a
/// long-lived background shell waits up to half an hour; the driver is
/// `Idle` for the whole of it, so Stop refuses it; and the steer that says
/// "now" used to answer "that message asks for different settings" and leave
/// everything exactly as it was. The message could only be abandoned.
///
/// So from idle the steer *is* the restart — background work and all, which
/// is the trade the confirmation in the UI is there to put to the user.
#[tokio::test]
async fn a_steer_from_idle_restarts_for_the_settings_it_asks_for() {
    let harness = Harness::start_attached(
        &history_with_an_orphan_task(),
        hold_until_released(),
        Duration::from_secs(3600),
        // Deliberately no watchdog inside this test's lifetime. The ceiling
        // is the *other* release, and a test about the button must not be
        // able to pass because a timer let the message through instead.
        Duration::from_secs(3600),
        Duration::from_millis(120),
        Witness::Fresh,
    )
    .await;
    until("the replay to settle", || harness.state().phase == Phase::Idle).await;
    assert_eq!(harness.state().background, 1, "there is real background work here to be traded away");

    harness.send_with("under a different model", harness.recipe(a_settings_change()));
    until("the settings change to park", || harness.state().awaiting_background).await;
    assert!(harness.after().is_empty(), "and nothing was restarted for it on its own");

    let qid = harness.qid("different model");
    assert_eq!(
        harness.steer(&qid).await,
        QueueOutcome::Done,
        "the press is taken at its word: from idle there is a process to replace"
    );

    until("the replacement to take the message", || harness.after().len() == 1).await;
    assert!(
        harness.after()[0].contains("different model"),
        "the message that ended the park is the one that ran: {:?}",
        harness.after()
    );
    assert_eq!(harness.spawns().len(), 1, "a process the driver started itself, not a second stdin line");
    assert_eq!(
        harness.recorded_fingerprint().and_then(|fingerprint| fingerprint.model),
        Some("opus".to_string()),
        "and it is the process the message asked for, as the next server would read it"
    );
    assert!(!harness.state().awaiting_background, "the park is over");
    assert_eq!(harness.state().queued, 0, "with nothing left waiting behind it");
    assert_eq!(
        harness.state().background,
        0,
        "the tasks went with the process the steer replaced, which is what the confirmation said would happen"
    );
}

/// The live bug: a directory's mtime says nothing about the files in it.
///
/// Claude creates a background agent's transcript when the agent starts and
/// then streams into it for as long as the agent runs. Writing to a file
/// that already exists does not touch its directory, so the `subagents/`
/// mtime freezes at the moment the last agent was *dispatched* while every
/// agent under it is still working — measured on a live session with two
/// running agents as a directory 133 seconds stale over transcripts written
/// that same second, well past the thirty-second window.
///
/// A probe that stats only the directory therefore calls a busy session
/// quiet and drops a task set that is entirely real, which is the worst
/// outcome this settle has: the work goes on, and nothing anywhere shows it.
#[tokio::test]
async fn a_task_still_streaming_into_an_old_directory_survives_the_settle() {
    let harness = Harness::start_attached(
        &history_with_an_orphan_task(),
        hold_until_released(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
        Witness::StreamingIntoAnOldDirectory,
    )
    .await;
    until("the replay to settle", || harness.state().phase == Phase::Idle).await;
    assert_eq!(
        harness.state().background,
        1,
        "a transcript written now corroborates the task however old the directory holding it is"
    );
    let kept = harness.state().tasks;
    assert_eq!(kept.len(), 1, "and the task is still named for the panel");
    assert_eq!(kept[0].id, "ghost");
}

/// The other half of the widened witness: a background *command* writes no
/// subagent transcript at all.
///
/// A `local_bash` task streams into `<task-id>.output` under the task root
/// and leaves `subagents/` untouched — there is no agent to have a
/// transcript. Reading only the transcript directory makes every such task a
/// ghost the moment its events fall into a replay gap, whatever it is doing.
#[tokio::test]
async fn a_background_command_is_corroborated_by_its_output_file() {
    let harness = Harness::start_attached(
        &history_with_an_orphan_task(),
        hold_until_released(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
        Witness::TaskOutputOnly,
    )
    .await;
    until("the replay to settle", || harness.state().phase == Phase::Idle).await;
    assert!(
        !crate::sessions::transcript::subagent_dir(&harness.transcript()).exists(),
        "the point of this witness is that there is no transcript directory to read"
    );
    assert_eq!(harness.state().background, 1, "a task output file written now corroborates the task");
}

/// And the drop still happens, which is what keeps the queue safe.
///
/// Reading the entries makes the probe more willing to believe a replayed
/// set, so the case it must still refuse is the one where the evidence
/// exists and is simply old: a transcript that stopped being written long
/// ago is an agent that is gone, and holding a settings change behind it is
/// the parking bug [`Driver::settle_background`] exists to prevent.
#[tokio::test]
async fn a_task_whose_transcript_stopped_long_ago_is_still_dropped() {
    let harness = Harness::start_attached(
        &history_with_an_orphan_task(),
        hold_until_released(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
        Witness::Stale,
    )
    .await;
    until("the replay to settle", || harness.state().phase == Phase::Idle).await;
    assert_eq!(harness.state().background, 0, "an old transcript is not a sign of life");
    assert!(harness.state().tasks.is_empty(), "and nothing is left to name");
    until("the recording to say so", || staged(&harness.recording(), crate::record::Stage::Settle)).await;

    // The proof it is a drop and not merely a different count: a settings
    // change goes straight through instead of parking behind the ghost.
    harness.send_with("under a different model", harness.recipe(a_settings_change()));
    until("the replacement to take the message", || harness.after().len() == 1).await;
    assert!(!harness.state().awaiting_background, "nothing was ever parked");
}

/// A restart takes claude's background work with it, whatever the outgoing
/// process had told this driver.
///
/// A replacement only announces its task list when the list *changes*, so a
/// set carried across a respawn is never contradicted: it would hold every
/// later settings change hostage to work that stopped existing.
#[tokio::test]
async fn a_replacement_process_inherits_no_background_tasks() {
    let harness = Harness::ordinary().await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    // A task started live — nothing provisional about this one.
    harness.plan_claude(&replacement_claude(SESSION, FORKED));
    harness.send("start something in the background");
    until("the turn", || harness.state().phase == Phase::Working).await;
    harness.release();
    until("the turn to end", || harness.state().phase == Phase::Idle).await;

    harness.send_with("under a different model", harness.recipe(a_settings_change()));
    until("the replacement to take the message", || harness.after().len() == 1).await;
    assert_eq!(harness.state().background, 0);
    assert!(!harness.state().awaiting_background);
}

// ---------------------------------------------------------- the ladder

/// The first rung ends the *turn*, not the process — which is the whole
/// reason the ladder starts where it does. A Stop that killed claude would
/// take every background task it had started with it.
#[tokio::test]
async fn an_interrupt_ends_the_turn_without_ending_the_process() {
    // A turn that never ends by itself, over a process that keeps reading
    // stdin — which is what makes the polite rung possible at all: the
    // request has to be *read* mid-turn to be answered mid-turn.
    let harness = Harness::start(
        r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
*control_request*)
   printf '%s\n' '{"type":"control_response","response":{"subtype":"success"}}'
   printf '%s\n' '{"type":"result","subtype":"interrupted","is_error":false}' ;;
*) : ;;
  esac
done
"#,
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    )
    .await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("something long");
    until("the turn", || harness.state().phase == Phase::Working).await;

    assert!(harness.registry.interrupt_resident(SESSION), "a working resident has a turn to stop");
    until("the turn to end", || harness.state().phase == Phase::Idle).await;

    let injection = harness.injection().expect("a run");
    assert_eq!(injection.state, InjectionRunState::Failed);
    assert!(injection.interrupted, "and it says it was interrupted");
    assert_eq!(injection.error.0.as_deref(), Some("interrupted from Droppen"));

    // The polite request is what was written, and the process took it.
    let seen = harness.seen();
    assert!(seen[1].contains("control_request"), "{seen:?}");
    assert!(seen[1].contains("\"subtype\":\"interrupt\""), "{seen:?}");

    // And an idle resident has nothing to stop.
    assert!(!harness.registry.interrupt_resident(SESSION));
    assert!(!harness.registry.interrupt_resident("no-such-session"));

    // The process is still there, and still listening — which is the whole
    // point of stopping a turn this way rather than with a signal.
    assert!(!harness.driver.is_finished());
    harness.send("and the session goes on");
    until("the next message to be read", || harness.seen().len() == 3).await;
}

/// The polite request, whole, uuid aside.
#[tokio::test]
async fn an_interrupt_reaches_claude_as_exactly_these_bytes() {
    let harness = Harness::start(
        r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
*control_request*)
   printf '%s\n' '{"type":"control_response","response":{"subtype":"success"}}'
   printf '%s\n' '{"type":"result","subtype":"interrupted","is_error":false}' ;;
*) : ;;
  esac
done
"#,
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    )
    .await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("something long");
    until("the turn", || harness.state().phase == Phase::Working).await;
    assert!(harness.registry.interrupt_resident(SESSION));
    until("the request to be read", || harness.seen().len() == 2).await;
    assert_eq!(
        masked_request_id(&harness.seen()[1], "int-"),
        r#"{"type":"control_request","request_id":"int-<uuid>","request":{"subtype":"interrupt"}}"#
    );
}

/// A CLI that will not answer gets signalled, rung by rung. The script traps
/// INT and TERM, so only the last rung can end it — which is what makes the
/// timing below evidence that all three ran.
#[tokio::test]
async fn a_wedged_turn_is_escalated_one_signal_at_a_time() {
    let step = Duration::from_millis(150);
    let harness = Harness::start(
        r#"
trap '' INT TERM
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  await_go
done
"#,
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        step,
    )
    .await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("wedge me");
    until("the turn", || harness.state().phase == Phase::Working).await;

    let started = Instant::now();
    assert!(harness.registry.interrupt_resident(SESSION));
    until("the wedged claude to be killed", || harness.driver.is_finished()).await;
    assert!(
        started.elapsed() >= step * 2,
        "SIGKILL is the third rung, not the first: {:?}",
        started.elapsed()
    );

    let injection = harness.injection().expect("a run");
    assert_eq!(injection.state, InjectionRunState::Failed);
    assert!(injection.interrupted);
    assert_eq!(
        injection.error.0.as_deref(),
        Some("interrupted from Droppen"),
        "a process this server killed on purpose is not reported as a crash"
    );
}

// --------------------------------------------------- the recovery ladder

/// A body that answers nothing: it logs the message, waits for the release,
/// and dies where a result should have been. What every recovery test needs
/// — a process that is unambiguously *mid-turn* when it goes.
pub(super) fn die_when_released() -> &'static str {
    r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  await_go
  rm -f "$GO"
  exit 9
done
"#
}

/// The first rung: a death mid-turn is a `--resume`, the lost turn is
/// reported as lost rather than silently retried, and the session carries
/// on in the replacement.
#[tokio::test]
async fn a_death_mid_turn_is_recovered_by_resume_and_the_turn_owns_up() {
    let harness = Harness::ordinary_with(die_when_released()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("doomed");
    until("the turn", || harness.state().phase == Phase::Working).await;
    harness.release();

    until("the resume", || harness.spawns().len() == 1).await;
    let spawned = harness.spawns().remove(0);
    assert!(spawned.contains(&format!("--resume {SESSION}")), "{spawned}");
    assert!(!spawned.contains("--fork-session"), "the first rung is a plain resume: {spawned}");

    // The turn is failed, not retried: its side effects already ran once.
    until("the turn to be owned up to", || {
        harness.injection().is_some_and(|run| run.state == InjectionRunState::Failed)
    })
    .await;
    let error = harness.injection().expect("a run").error.0.expect("a reason");
    assert!(error.contains("claude exited with code 9"), "{error}");
    assert!(error.contains("the session was recovered; this turn was not"), "{error}");

    // And the session is genuinely back: the replacement answers.
    harness.send("after the fact");
    until("the replacement to answer", || {
        harness.injection().is_some_and(|run| run.state == InjectionRunState::Ok)
    })
    .await;
    assert!(
        harness.after().iter().any(|line| line.contains("after the fact")),
        "{:?}",
        harness.after()
    );
    assert!(!harness.driver.is_finished(), "the session outlives the process");
}

/// The queue is the half that *is* redelivered: those messages never
/// reached the process that died, so running them in the replacement runs
/// them for the first time.
#[tokio::test]
async fn messages_queued_when_claude_died_are_delivered_to_its_replacement() {
    let harness = Harness::ordinary_with(die_when_released()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("doomed");
    until("the turn", || harness.state().phase == Phase::Working).await;
    harness.send("survivor");
    until("the queue", || harness.state().queued == 1).await;
    harness.release();

    until("the survivor to arrive", || {
        harness.after().iter().any(|line| line.contains("\"survivor\""))
    })
    .await;
    until("its turn to finish", || {
        harness.state().queued == 0 && harness.injection().is_some_and(|run| run.state == InjectionRunState::Ok)
    })
    .await;
    assert!(
        !harness.after().iter().any(|line| line.contains("\"doomed\"")),
        "the in-flight turn is not retried: {:?}",
        harness.after()
    );
}

/// The second rung, and the reason it exists: a replacement that dies
/// *before saying it is listening* makes the resume itself the suspect, so
/// the ladder forks — and then follows the id the fork answers under.
#[tokio::test]
async fn a_replacement_that_dies_before_listening_is_forked_and_followed() {
    let harness = Harness::ordinary_with(die_when_released()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    // The claude every recovery from here on finds: plain resumes are
    // refused before init, a fork is answered — under its own id, because
    // that is what `--fork-session` does.
    harness.plan_claude(&format!(
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$SPAWNS"
case "$*" in
  *--fork-session*) : ;;
  *) exit 91 ;;
esac
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{FORKED}"}}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$AFTER"
  case "$line" in
*control_request*) : ;;
*) printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}' ;;
  esac
done
"#
    ));
    harness.send("doomed");
    until("the turn", || harness.state().phase == Phase::Working).await;
    harness.release();

    until("the ladder to reach the fork", || harness.spawns().len() == 3).await;
    let spawns = harness.spawns();
    assert!(spawns[0].contains("--resume") && !spawns[0].contains("--fork-session"), "{spawns:?}");
    assert!(spawns[1].contains("--resume") && !spawns[1].contains("--fork-session"), "{spawns:?}");
    assert!(spawns[2].contains("--fork-session"), "{spawns:?}");

    // The registry followed the fork: the session answers under the new id.
    until("the fork to be adopted", || {
        harness.registry.residents.lock().expect("residents").contains_key(FORKED)
    })
    .await;
    harness.send("in the fork");
    until("the forked session to answer", || {
        harness.injection_for(FORKED).is_some_and(|run| run.state == InjectionRunState::Ok)
    })
    .await;
    assert!(harness.after().iter().any(|line| line.contains("in the fork")), "{:?}", harness.after());
}

/// The breaker: a session that cannot be brought back stops being retried,
/// and the error says what was tried rather than how it went missing.
#[tokio::test]
async fn a_crash_loop_trips_the_breaker_and_says_what_was_tried() {
    let harness = Harness::ordinary_with(die_when_released()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.refuse_to_start();
    harness.send("doomed");
    until("the turn", || harness.state().phase == Phase::Working).await;
    harness.release();

    until("the breaker to trip", || harness.driver.is_finished()).await;
    assert_eq!(harness.spawns().len(), 3, "resume, resume, fork — and no fourth: {:?}", harness.spawns());
    let error = harness.injection().expect("a run").error.0.expect("a reason");
    assert!(error.contains("could not be recovered"), "{error}");
    assert!(error.contains("--resume --fork-session"), "the error names the rungs: {error}");
    assert!(
        !harness.registry.residents.lock().expect("residents").contains_key(SESSION),
        "a session the breaker gave up on is not still claimed"
    );
}

// ------------------------------------------------------- the fingerprint

/// A message the running process cannot take restarts it, and the message
/// waits for the replacement rather than going to the wrong one.
///
/// The replacement is a real spawn through a real shim over a fake `claude`
/// on the `PATH` this recipe carries, because the thing worth pinning is
/// that the whole path works — the polite shutdown of the old process, the
/// fresh sidecars a new shim needs, the journal entry moving across, and the
/// message arriving in the replacement rather than the process it outgrew.
#[tokio::test]
async fn a_message_that_does_not_fit_the_running_process_gets_a_new_one() {
    let harness = Harness::ordinary().await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    // The `claude` the replacement will find, and the files it reports through.
    let bin = harness._home.path().join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    let after = harness._home.path().join("after.log");
    std::fs::write(
        bin.join("claude"),
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"{SESSION}\"}}'\n\
             while IFS= read -r line; do\n  printf '%s\\n' \"$line\" >> \"$AFTER\"\n  \
             printf '%s\\n' '{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}}'\ndone\n"
        ),
    )
    .expect("fake claude");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    let mut env = Env::new();
    env.insert("PATH".into(), format!("{}:/bin:/usr/bin", bin.display()));
    env.insert("AFTER".into(), after.to_string_lossy().into_owned());
    harness.send_with(
        "under a different model",
        Recipe {
            args: vec!["--model".into(), "opus".into()],
            env,
            workdir: Some(harness._home.path().to_path_buf()),
            fingerprint: Fingerprint {
                model: Some("opus".into()),
                ..Default::default()
            },
        },
    );

    until("the replacement to take the message", || {
        std::fs::read_to_string(&after).is_ok_and(|log| log.contains("under a different model"))
    })
    .await;
    assert!(
        !harness.seen().iter().any(|line| line.contains("under a different model")),
        "the message must never reach the process it did not fit"
    );
    until("the turn to finish on the new process", || harness.state().phase == Phase::Idle).await;
    assert_eq!(harness.injection().expect("a run").state, InjectionRunState::Ok);

    // One record, for the process that is actually running, pointing at the
    // shim a later boot would have to re-attach through.
    let records = harness.registry.host.journal.read();
    assert_eq!(records.len(), 1, "{records:?}");
    assert!(records[0].resident);
    assert!(records[0].shim.is_some(), "a resident record names its shim");
    assert_ne!(records[0].run_id, "record-under-test", "the retired record went with the old process");

    // The recording followed the process across: the restart is in it, and
    // so is the spawn that answered it — with the settings that caused it
    // and, emphatically, without the environment that carried the token.
    let events = harness.recording();
    assert!(staged(&events, crate::record::Stage::Respawn), "{events:?}");
    let spawn = events
        .iter()
        .find_map(|event| match event {
            crate::record::Event::Spawned { selector, model, cwd, .. } => Some((selector.clone(), model.clone(), cwd.clone())),
            _ => None,
        })
        .expect("the replacement's spawn line");
    assert_eq!(spawn.0, "resume", "a replacement always resumes the transcript it inherits");
    assert_eq!(spawn.1.as_deref(), Some("opus"), "and says what it was started under");
    assert!(
        spawn.2.is_some(),
        "and where it runs, which is what replay derives a project directory from"
    );
}

/// The production failure, at the level it happened.
///
/// A resident spawned before the journal recorded fingerprints, still alive
/// after the server it belonged to was upgraded, with a message waiting
/// behind it. Every such message used to be answered with "the settings this
/// message needs were lost when the server restarted" — because a record
/// that said nothing was read as a process started with no flags at all,
/// which nothing an account-bound session ever sends can satisfy.
///
/// Unknown means unknown: the server that accepted these messages compared
/// them against the fingerprint it had, and this one, knowing less, does not
/// overrule it.
#[tokio::test]
async fn a_resident_with_no_recorded_settings_delivers_the_backlog_it_inherited() {
    let waiting = Queued {
        id: "q-1".to_string(),
        text: "still waiting from before the restart".to_string(),
        images: Vec::new(),
        files: Vec::new(),
        fingerprint: Fingerprint {
            model: Some("opus".into()),
            account: Some("work".into()),
            ..Default::default()
        },
        steered: false,
        rotation_attempt: 0,
        continuation: None,
    };
    let harness = Harness::start_inherited(None, vec![waiting]).await;

    until("the inherited message to reach claude", || {
        harness.seen().iter().any(|line| line.contains("still waiting from before the restart"))
    })
    .await;
    harness.release();
    until("the turn to end", || harness.state().phase == Phase::Idle).await;
    assert_eq!(
        harness.injection().expect("a run").state,
        InjectionRunState::Ok,
        "the message ran, rather than being failed for settings nobody wrote down"
    );
    // And nothing was taken from it: a message out of the journal has no
    // environment behind it, so it describes no process this driver could
    // start and is no evidence about the one it is holding.
    assert!(
        harness.recorded_fingerprint().is_none(),
        "a message with no recipe leaves the unknown where it was"
    );
}

/// A fresh send to a resident whose settings were never recorded takes the
/// process over rather than replacing it — and says what it is, once.
///
/// Both halves matter. Restarting would kill whatever the session was
/// minding in the background over a comparison against a fingerprint that
/// was never written down; and an unknown that stayed unknown would have to
/// be guessed at again after every restart, which is how one bad record
/// outlived three upgrades.
#[tokio::test]
async fn a_fresh_message_takes_over_a_resident_whose_settings_were_never_recorded() {
    let harness = Harness::start_inherited(None, Vec::new()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    let asked = Fingerprint {
        model: Some("opus".into()),
        account: Some("work".into()),
        ..Default::default()
    };
    harness.send_with("first message after the upgrade", harness.recipe(asked.clone()));
    until("the message to reach the process that is already running", || {
        harness.seen().iter().any(|line| line.contains("first message after the upgrade"))
    })
    .await;
    assert!(
        harness.spawns().is_empty(),
        "a process whose settings nobody recorded is not one to throw away: {:?}",
        harness.spawns()
    );
    harness.release();
    until("the turn to end", || harness.state().phase == Phase::Idle).await;

    // Taken once, and written where the *next* server reads it.
    assert_eq!(
        harness.recorded_fingerprint(),
        Some(asked.clone()),
        "the settings of the message that went in heal the record"
    );

    // Which is what puts the check back: the next message that really does
    // need a different process gets one.
    harness.send_with(
        "and now under another model",
        harness.recipe(Fingerprint {
            model: Some("haiku".into()),
            account: Some("work".into()),
            ..Default::default()
        }),
    );
    until("the replacement to take the message", || {
        harness.after().iter().any(|line| line.contains("and now under another model"))
    })
    .await;
    assert!(
        !harness.seen().iter().any(|line| line.contains("and now under another model")),
        "and the process it did not fit never sees it"
    );
}

/// #446, set by an account: `ANTHROPIC_MODEL=deepseek-v4.1-flash[1m]` and
/// no `--model`, so nothing but the process' `system:init` names its
/// model, marker and all (claude 2.1.312). That name is what the feed reads
/// the session's window off, so the mirror carries it.
#[tokio::test]
async fn the_mirror_names_the_model_as_the_cli_answers_for_it() {
    let history = format!(r#"printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}","model":"deepseek-v4.1-flash[1m]"}}'"#);
    let harness = Harness::start_attached(
        &history,
        hold_until_released(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
        Witness::None,
    )
    .await;
    let running = |id: &str| harness.providers().resident_state(Source::Claude, id).and_then(|state| state.model);
    until("claude's name for its model to reach the mirror", || running(SESSION).as_deref() == Some("deepseek-v4.1-flash[1m]")).await;
    assert_eq!(running("no-such-session"), None, "a session with no process here has no model to tell");
}

/// `init` opens every turn with the model in force for it, so a `/model`
/// sent as a message is followed at the next turn (claude 2.1.312). A
/// switch back to the name the process was spawned with is followed too,
/// or the process would be described by a model it no longer runs.
#[tokio::test]
async fn a_model_switched_inside_the_process_is_followed_there_and_back() {
    let body = format!(
        r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
*glm-5*) printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}","model":"glm-5"}}' ;;
*) printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}","model":"deepseek-v4.1-flash[1m]"}}' ;;
  esac
  printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}'
done
"#
    );
    let asked = Fingerprint {
        model: Some("deepseek-v4.1-flash[1m]".into()),
        ..Default::default()
    };
    let harness = Harness::build(
        &preamble(),
        &body,
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
        NEVER_SETTLES,
        Adoption {
            live: Some(asked),
            ..Adoption::default()
        },
    )
    .await;
    let running = || harness.state().model;
    until("the init", || harness.state().phase == Phase::Idle).await;
    for (message, model) in [
        ("first", "deepseek-v4.1-flash[1m]"),
        ("/model glm-5", "glm-5"),
        ("/model deepseek-v4.1-flash[1m]", "deepseek-v4.1-flash[1m]"),
    ] {
        let read = harness.seen().len();
        harness.send(message);
        until("the turn to be read and answered", || {
            harness.seen().len() > read && harness.state().phase == Phase::Idle && running().as_deref() == Some(model)
        })
        .await;
    }
}

/// A resident answers to both of its model names, and writes the second
/// one down.
///
/// The failure that produced the report this whole change came from. A
/// spawn asks for `--model fable`; `system:init` answers `claude-fable-5`;
/// the driver replaced the name it asked for with the name it was given,
/// and every send still saying `fable` then read as a settings change. The
/// recording shows what that cost: eight turns, eight restarts, the
/// session's background work killed each time, and the message queued
/// behind one of them failed with "the settings this message needs were
/// lost when the server restarted" — because a message out of the journal
/// has no environment to start a process of its own with.
///
/// Both names, then, and both on the record: the next server inherits the
/// pair rather than re-learning the second one at the price of a restart.
#[tokio::test]
async fn a_resident_answers_to_both_its_model_names_and_records_the_second() {
    let history = format!(r#"printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}","model":"claude-fable-5"}}'"#);
    let asked = Fingerprint {
        model: Some("fable".into()),
        account: Some("work".into()),
        ..Default::default()
    };
    let harness = Harness::build(
        &history,
        hold_until_released(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
        NEVER_SETTLES,
        Adoption {
            // An attach, so the history — and the `init` in it — is replayed
            // before anything is pumped: that ordering is the scenario.
            attach: Some(Witness::None),
            live: Some(asked.clone()),
            queued: vec![Queued {
                id: "q-1".to_string(),
                text: "queued under the name the composer sends".to_string(),
                images: Vec::new(),
                files: Vec::new(),
                fingerprint: asked.clone(),
                steered: false,
                rotation_attempt: 0,
                continuation: None,
            }],
            recipe: None,
            hold: None,
            ring: None,
            transcript: None,
            ..Adoption::default()
        },
    )
    .await;

    // The inherited message goes in rather than being failed for a name the
    // CLI chose after it was queued.
    until("the inherited message to reach claude", || {
        harness
            .seen()
            .iter()
            .any(|line| line.contains("queued under the name the composer sends"))
    })
    .await;
    harness.release();
    until("the turn to end", || harness.state().phase == Phase::Idle).await;
    assert_eq!(harness.injection().expect("a run").state, InjectionRunState::Ok);

    // Both names are on the record, so a restart from here compares against
    // either one.
    assert_eq!(
        harness.recorded_fingerprint(),
        Some(Fingerprint {
            resolved: Some("claude-fable-5".into()),
            ..asked.clone()
        }),
        "the name the CLI answered under joins the one the spawn asked for"
    );

    // And a send naming the *other* one is the same process too: no restart,
    // and nothing of what the session was doing is thrown away for it.
    harness.send_with(
        "and now under the name the CLI answers with",
        harness.recipe(Fingerprint {
            model: Some("claude-fable-5".into()),
            ..asked
        }),
    );
    until("the second message to reach the same process", || {
        harness
            .seen()
            .iter()
            .any(|line| line.contains("and now under the name the CLI answers with"))
    })
    .await;
    assert!(harness.spawns().is_empty(), "neither name is a settings change: {:?}", harness.spawns());
}

/// The safety net, at the one place a message out of the journal can still
/// meet a process it does not fit.
///
/// The process this driver inherited dies, and the replacement it starts is
/// built from the only recipe it has — which came from a message that named
/// nothing in particular. That is a *different* process from the one the
/// backlog was queued against, and the message left in the queue has no
/// environment of its own to start the one it wants. Delivering it anyway
/// would run somebody's prompt under a model they did not choose and bill it
/// to an account they did not pick, so it is failed instead — and told
/// exactly which settings are in the way, because "send it again" is a
/// perfectly good answer and "the settings were lost" is not.
#[tokio::test]
async fn a_message_out_of_the_journal_is_failed_rather_than_run_by_the_wrong_process() {
    let wanted = Fingerprint {
        model: Some("opus".into()),
        account: Some("work".into()),
        ..Default::default()
    };
    let queued = |id: &str, text: &str| Queued {
        id: id.to_string(),
        text: text.to_string(),
        images: Vec::new(),
        files: Vec::new(),
        fingerprint: wanted.clone(),
        steered: false,
        rotation_attempt: 0,
        continuation: None,
    };
    let harness = Harness::build(
        &preamble(),
        die_when_released(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
        NEVER_SETTLES,
        Adoption {
            attach: None,
            live: Some(wanted.clone()),
            queued: vec![queued("q-1", "the one in flight"), queued("q-2", "the one behind it")],
            recipe: None,
            hold: None,
            ring: None,
            transcript: None,
            ..Adoption::default()
        },
    )
    .await;

    until("the first message to be taken", || harness.state().phase == Phase::Working).await;
    // The process dies holding it, and the recovery ladder starts a
    // replacement from the recipe this driver has: the harness' own, which
    // names no model and no account.
    harness.release();

    until("the second message to be answered", || {
        harness.injection().is_some_and(|run| run.state == InjectionRunState::Failed) && harness.state().queued == 0
    })
    .await;
    let error = harness.injection().expect("a run").error.0.expect("a reason");
    assert!(
        error.contains("model opus rather than none") && error.contains("the work account rather than the local login"),
        "the failure names every setting in the way: {error}"
    );
    assert!(error.contains("send it again"), "and what to do about it: {error}");
    assert!(
        !harness.after().iter().any(|line| line.contains("the one behind it")),
        "and the replacement it did not fit never sees it: {:?}",
        harness.after()
    );
}
