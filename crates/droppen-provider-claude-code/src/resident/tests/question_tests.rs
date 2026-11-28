//! Interactive questions and tool permission tests.

use super::*;

// ------------------------------------------------------------ questions

/// The two questions every question test asks, as the tool input carries
/// them — and therefore exactly what an answer has to echo.
fn colours_and_toppings() -> Value {
    serde_json::json!([
        {"question": "Which colour do you prefer?", "header": "Colour",
         "options": [{"label": "Red", "description": "Warm"}, {"label": "Blue", "description": "Cool"}],
         "multiSelect": false},
        {"question": "Which toppings?", "header": "Toppings",
         "options": [{"label": "Cheese"}, {"label": "Olives"}],
         "multiSelect": true}
    ])
}

/// The `assistant` line carrying the call and the `can_use_tool` request,
/// as claude 2.1.295 writes them, for a script to `printf`.
fn question_lines() -> [String; 2] {
    let request = serde_json::json!({
        "type": "control_request",
        "request_id": "req-q1",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "AskUserQuestion",
            "display_name": "AskUserQuestion",
            "input": {"questions": colours_and_toppings()},
            "tool_use_id": "toolu_q1",
            "requires_user_interaction": true,
        },
    });
    [
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_q1","name":"AskUserQuestion","input":{}}]}}"#.to_string(),
        request.to_string(),
    ]
}

/// The CLI with the prompt tool switched on: every message is answered with
/// a question, and the turn then waits — with no timeout, as the real one
/// does — for the response on stdin. The answer is echoed as the tool
/// result the CLI builds from it; an interrupt is answered the way 2.1.295
/// answers one with a question open: the request withdrawn, the turn
/// failed.
fn asks_a_question() -> String {
    let [call, request] = question_lines();
    format!(
        r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
*control_response*)
   printf '%s\n' '{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_q1","content":"User has answered your questions"}}]}}}}'
   printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"thanks"}}]}}}}'
   printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}' ;;
*control_request*)
   printf '%s\n' '{{"type":"control_cancel_request","request_id":"req-q1"}}'
   printf '%s\n' '{{"type":"result","subtype":"error_during_execution","is_error":true}}' ;;
*)
   printf '%s\n' '{call}'
   printf '%s\n' '{request}' ;;
  esac
done
"#
    )
}

/// The line the driver wrote to answer `request_id`, parsed.
fn response_to(seen: &[String], request_id: &str) -> Option<Value> {
    seen.iter()
        .filter(|line| line.contains("control_response"))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|line| line.pointer("/response/request_id").and_then(Value::as_str) == Some(request_id))
}

fn blue_with_cheese_and_olives() -> questions::Answer {
    questions::Answer {
        answers: std::collections::BTreeMap::from([
            ("Which colour do you prefer?".to_string(), "Blue".to_string()),
            (
                "Which toppings?".to_string(),
                questions::join_choices(&["Cheese".to_string(), "Olives".to_string()]),
            ),
        ]),
        ..questions::Answer::default()
    }
}

/// The feature, end to end through the driver: the question holds the
/// turn open — and the session *asking* while it is working — the answer
/// goes back on stdin in the one shape the CLI takes, and the turn goes on
/// to its own end.
#[tokio::test]
async fn a_question_holds_the_turn_open_and_its_answer_goes_back_on_stdin() {
    let harness = Harness::ordinary_with(&asks_a_question()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("pick for me");
    until("the question", || harness.state().question.is_some()).await;

    assert_eq!(harness.state().question.as_deref(), Some("req-q1"));
    assert_eq!(harness.state().phase, Phase::Working, "claude asks mid-turn, and the turn waits");
    let info: droppen_sessions::ResidentInfo = harness.state().into();
    assert!(info.asking, "and the row says it is asking, while working");
    assert_eq!(harness.registry.open_question(SESSION).as_deref(), Some("req-q1"));
    let stored = harness.registry.host.questions().get("req-q1").expect("on record");
    assert_eq!((stored.state, stored.tool_use_id.as_deref()), (QuestionState::Pending, Some("toolu_q1")));
    assert_eq!(stored.questions, colours_and_toppings());

    // An answer to something that was not asked is refused, and changes
    // nothing.
    let wrong = questions::Answer {
        answers: std::collections::BTreeMap::from([("which colour?".to_string(), "Blue".to_string())]),
        ..questions::Answer::default()
    };
    assert!(matches!(
        harness.registry.answer_question(SESSION, "req-q1", Reply::Answer(wrong), None).await,
        AnswerOutcome::Invalid(_)
    ));
    assert!(response_to(&harness.seen(), "req-q1").is_none());

    let delivered = harness
        .registry
        .answer_question(SESSION, "req-q1", Reply::Answer(blue_with_cheese_and_olives()), Some("phone".into()))
        .await;
    let AnswerOutcome::Delivered(question) = delivered else {
        panic!("delivered: {delivered:?}")
    };
    assert_eq!(
        (question.state, question.answered_by.0.as_deref()),
        (QuestionState::Answered, Some("phone"))
    );

    until("the turn to go on and end", || harness.state().phase == Phase::Idle).await;
    let line = response_to(&harness.seen(), "req-q1").expect("the answer went in");
    assert_eq!(line["response"]["subtype"], "success");
    let response = &line["response"]["response"];
    assert_eq!(response["behavior"], "allow");
    assert_eq!(response["updatedInput"]["questions"], colours_and_toppings(), "echoed verbatim");
    assert_eq!(response["updatedInput"]["answers"]["Which colour do you prefer?"], "Blue");
    assert_eq!(response["updatedInput"]["answers"]["Which toppings?"], "Cheese, Olives");
    assert_eq!(harness.state().question, None);
    assert_eq!(harness.injection().map(|run| run.state), Some(InjectionRunState::Ok));

    // A second answer — another screen, a double tap — reaches nothing:
    // the driver is no longer holding the question, and the record says
    // who got there first.
    assert!(matches!(
        harness
            .registry
            .answer_question(SESSION, "req-q1", Reply::Answer(blue_with_cheese_and_olives()), Some("laptop".into()))
            .await,
        AnswerOutcome::Unknown
    ));
    let stored = harness.registry.host.questions().get("req-q1").expect("on record");
    assert_eq!((stored.state, stored.answered_by.as_deref()), (QuestionState::Answered, Some("phone")));
    assert_eq!(
        harness.seen().iter().filter(|line| line.contains("control_response")).count(),
        1,
        "one answer, once"
    );
}

/// "Use your judgement": refused, in words the agent can carry on from.
#[tokio::test]
async fn a_dismissed_question_is_refused_and_the_turn_goes_on() {
    let harness = Harness::ordinary_with(&asks_a_question()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("pick for me");
    until("the question", || harness.state().question.is_some()).await;
    let outcome = harness.registry.answer_question(SESSION, "req-q1", Reply::Dismiss, None).await;
    assert!(
        matches!(&outcome, AnswerOutcome::Delivered(question) if question.state == QuestionState::Dismissed),
        "{outcome:?}"
    );
    until("the turn to end", || harness.state().phase == Phase::Idle).await;
    let line = response_to(&harness.seen(), "req-q1").expect("the refusal went in");
    assert_eq!(line["response"]["response"], questions::deny(questions::DISMISSED));
}

/// The questions [`asks_a_question`] asks, in the bytes its request line
/// carries them in, which is what an answer echoes.
const COLOURS_AND_TOPPINGS: &str = r#"[{"question":"Which colour do you prefer?","header":"Colour","options":[{"label":"Red","description":"Warm"},{"label":"Blue","description":"Cool"}],"multiSelect":false},{"question":"Which toppings?","header":"Toppings","options":[{"label":"Cheese"},{"label":"Olives"}],"multiSelect":true}]"#;

/// An answer with notes, as the line claude reads, whole: key order is
/// part of it, and a parsed `Value` compares equal in any order. Words
/// beside the choices ride as a note on the last question answered.
#[tokio::test]
async fn an_answer_with_notes_reaches_claude_as_exactly_these_bytes() {
    let harness = Harness::ordinary_with(&asks_a_question()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("pick for me");
    until("the question", || harness.state().question.is_some()).await;
    let answer = questions::Answer {
        notes: std::collections::BTreeMap::from([
            ("Which colour do you prefer?".to_string(), "for the header".to_string()),
            ("Which toppings?".to_string(), "no \"extra\" cheese".to_string()),
        ]),
        response: Some("thanks — 谢谢".to_string()),
        ..blue_with_cheese_and_olives()
    };
    let outcome = harness.registry.answer_question(SESSION, "req-q1", Reply::Answer(answer), None).await;
    assert!(matches!(outcome, AnswerOutcome::Delivered(_)), "{outcome:?}");
    until("the turn to end", || harness.state().phase == Phase::Idle).await;
    assert_eq!(
        harness.seen()[1..],
        [[
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-q1","response":{"behavior":"allow","updatedInput":{"questions":"#,
            COLOURS_AND_TOPPINGS,
            r#","answers":{"Which colour do you prefer?":"Blue","Which toppings?":"Cheese, Olives"},"#,
            r#""annotations":{"Which colour do you prefer?":{"notes":"for the header"},"Which toppings?":{"notes":"no \"extra\" cheese; thanks — 谢谢"}}}}}}"#,
        ]
        .concat()]
    );
}

/// Words typed in place of a choice, as the line claude reads, whole:
/// `answers` still there and empty, the words trimmed as `response`, and
/// no `annotations`.
#[tokio::test]
async fn a_typed_response_reaches_claude_as_exactly_these_bytes() {
    let harness = Harness::ordinary_with(&asks_a_question()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("pick for me");
    until("the question", || harness.state().question.is_some()).await;
    let said = questions::Answer {
        response: Some("  just pick one  ".to_string()),
        ..questions::Answer::default()
    };
    let outcome = harness.registry.answer_question(SESSION, "req-q1", Reply::Answer(said), None).await;
    assert!(matches!(outcome, AnswerOutcome::Delivered(_)), "{outcome:?}");
    until("the turn to end", || harness.state().phase == Phase::Idle).await;
    assert_eq!(
        harness.seen()[1..],
        [[
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-q1","response":{"behavior":"allow","updatedInput":{"questions":"#,
            COLOURS_AND_TOPPINGS,
            r#","answers":{},"response":"just pick one"}}}}"#,
        ]
        .concat()]
    );
}

/// A dismissed question, as the line claude reads, whole.
#[tokio::test]
async fn a_dismissal_reaches_claude_as_exactly_these_bytes() {
    let harness = Harness::ordinary_with(&asks_a_question()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("pick for me");
    until("the question", || harness.state().question.is_some()).await;
    let outcome = harness.registry.answer_question(SESSION, "req-q1", Reply::Dismiss, None).await;
    assert!(matches!(outcome, AnswerOutcome::Delivered(_)), "{outcome:?}");
    until("the turn to end", || harness.state().phase == Phase::Idle).await;
    assert_eq!(
        harness.seen()[1..],
        [
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-q1","response":{"behavior":"deny","message":"The user chose not to answer this question. Use your own judgement and carry on."}}}"#
        ]
    );
}

/// A non-question `can_use_tool` prompt is held open for interactive approval
/// and delivers the chosen `allow` (or `allow_always` with suggestions) back
/// on stdin so the turn continues.
#[tokio::test]
async fn every_other_permission_prompt_is_held_and_answered_interactively() {
    let request = serde_json::json!({
        "type": "control_request",
        "request_id": "req-w1",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "Write",
            "input": {"file_path": "/etc/hosts", "content": "x"},
            "tool_use_id": "toolu_w1",
            "decision_reason": "outside the workspace",
            "permission_suggestions": [{"type": "addRules", "rules": [{"toolName": "Write"}]}],
        },
    });
    let harness = Harness::ordinary_with(&format!(
        r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
*control_response*)
   printf '%s\n' '{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_w1","content":"written"}}]}}}}'
   printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}' ;;
*control_request*) : ;;
*) printf '%s\n' '{request}' ;;
  esac
done
"#
    ))
    .await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("write it to /etc/hosts");
    until("the pending permission", || harness.state().permission.is_some()).await;
    assert_eq!(harness.state().permission.as_deref(), Some("req-w1"));
    assert!(harness.state().question.is_none());
    let info: droppen_sessions::ResidentInfo = harness.state().into();
    assert!(info.asking, "asking while waiting on permission approval");
    assert!(
        harness.registry.host.questions().get("req-w1").is_none(),
        "a permission prompt is not a question"
    );
    let stored = harness.registry.host.permissions().get("req-w1").expect("recorded in permissions");
    assert_eq!(stored.state, PermissionState::Pending);
    assert_eq!(stored.options.len(), 3, "allow, always allow (from suggestions), deny");

    let outcome = harness
        .registry
        .respond_permission(SESSION, "req-w1", PermissionReply::Select(permissions::CLAUDE_ALLOW_ALWAYS.into()), Some("alice".into()))
        .await;
    assert!(matches!(outcome, PermissionOutcome::Delivered(_)), "{outcome:?}");
    until("the turn's end", || harness.state().phase == Phase::Idle).await;
    let line = response_to(&harness.seen(), "req-w1").expect("answered");
    let response = &line["response"]["response"];
    assert_eq!(response["behavior"], "allow");
    assert_eq!(response["updatedInput"], serde_json::json!({"file_path": "/etc/hosts", "content": "x"}));
    assert_eq!(response["updatedPermissions"], serde_json::json!([{"type": "addRules", "rules": [{"toolName": "Write"}]}]));
    assert_eq!(harness.injection().map(|run| run.state), Some(InjectionRunState::Ok));
}

/// A denied permission prompt and a declined unknown control request reach
/// claude as exactly these bytes.
#[tokio::test]
async fn a_refusal_and_a_declined_request_reach_claude_as_exactly_these_bytes() {
    let write = r#"{"type":"control_request","request_id":"req-w1","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"file_path":"/etc/hosts","content":"x"},"tool_use_id":"toolu_w1","decision_reason":"outside the workspace"}}"#;
    let hook = r#"{"type":"control_request","request_id":"hook-1","request":{"subtype":"hook_callback","callback_id":"x"}}"#;
    let harness = Harness::ordinary_with(&format!(
        r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
*control_response*) : ;;
*) printf '%s\n' '{write}'
   printf '%s\n' '{hook}' ;;
  esac
done
"#
    ))
    .await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("write it to /etc/hosts");
    until("hook declined and permission pending", || {
        harness.seen().len() == 2 && harness.state().permission.as_deref() == Some("req-w1")
    })
    .await;
    let outcome = harness
        .registry
        .respond_permission(SESSION, "req-w1", PermissionReply::Select(permissions::CLAUDE_DENY.into()), None)
        .await;
    assert!(matches!(outcome, PermissionOutcome::Delivered(_)), "{outcome:?}");
    until("both to be answered", || harness.seen().len() == 3).await;
    assert_eq!(
        harness.seen()[1..],
        [
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"hook-1","error":"Droppen does not handle this request"}}"#,
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-w1","response":{"behavior":"deny","message":"The user denied permission to use Write."}}}"#,
        ]
    );
}

/// The two clocks that would throw a question's turn away, and must not.
///
/// claude writes nothing while it waits — that is what blocked means — so
/// the stall watchdog sees a silent turn; and the transcript, asked whether
/// a turn is in flight, reads a tail ending on `AskUserQuestion` as a
/// session waiting for input. Before the exemption, the two together let
/// go of the turn after `DROPPEN_STALL_MS` and pushed the queue into the middle
/// of it.
#[tokio::test]
async fn a_question_is_never_taken_for_a_stall_or_a_turn_that_ended() {
    let harness = Harness::start(
        &asks_a_question(),
        Duration::from_millis(60),
        Duration::from_millis(30),
        Duration::from_secs(3600),
    )
    .await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("pick for me");
    until("the question", || harness.state().question.is_some()).await;
    // The transcript as claude leaves it at a question, settled long ago.
    let transcript = harness.transcript();
    std::fs::write(
        &transcript,
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu_q1\",\"name\":\"AskUserQuestion\",\"input\":{}}]}}\n",
    )
    .expect("transcript");
    long_ago(&transcript);
    assert_eq!(
        crate::sessions::claude::claude_activity_status(&transcript).await,
        SessionStatus::Waiting,
        "the transcript would call this between turns"
    );
    // A message sent meanwhile waits behind the question's turn.
    harness.send("queued behind it");

    tokio::time::sleep(Duration::from_millis(400)).await;
    let state = harness.state();
    assert_eq!(state.phase, Phase::Working, "the turn is still in flight");
    assert!(!state.stalled, "waiting on the user is not being stuck");
    assert_eq!(state.question.as_deref(), Some("req-q1"));
    assert_eq!(state.queued, 1, "and the queue did not go into the middle of it");
    assert_eq!(harness.injection().map(|run| run.state), Some(InjectionRunState::Running));
    assert_eq!(harness.seen().len(), 1, "nothing else was written to claude");

    harness
        .registry
        .answer_question(SESSION, "req-q1", Reply::Answer(blue_with_cheese_and_olives()), None)
        .await;
    until("the queued message to go in after the answer", || {
        harness.seen().iter().any(|line| line.contains("queued behind it"))
    })
    .await;
}

/// A stop takes the question back with the turn: nobody may answer it into
/// whatever comes next.
#[tokio::test]
async fn a_stop_cancels_the_question_it_was_blocked_on() {
    // No rung in the test's lifetime: the stop under test is the
    // request claude answers. A rung that fired first, on a slow
    // machine, would signal claude before it had read the request —
    // and the process that asked the question would die, taking its
    // resident with it, so the answer afterwards would be `NoResident`
    // rather than the `Unknown` a cancelled question deserves.
    let harness = Harness::start(&asks_a_question(), NEVER_SETTLES, NEVER_SETTLES, NEVER_SETTLES).await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("pick for me");
    until("the question", || harness.state().question.is_some()).await;

    assert!(harness.registry.interrupt_resident(SESSION));
    until("the turn to end", || harness.state().phase == Phase::Idle).await;
    assert_eq!(harness.state().question, None);
    assert_eq!(
        harness.registry.host.questions().get("req-q1").map(|stored| stored.state),
        Some(QuestionState::Cancelled)
    );
    assert!(harness.injection().is_some_and(|run| run.interrupted));
    assert!(matches!(
        harness.registry.answer_question(SESSION, "req-q1", Reply::Dismiss, None).await,
        AnswerOutcome::Unknown
    ));
    assert!(response_to(&harness.seen(), "req-q1").is_none(), "no answer followed the stop");
}

/// A question is a promise from the process that asked it. When that
/// process dies, the replacement `--resume`s with the call closed as
/// interrupted and never asks again — so the question expires, and stays on
/// record to be answered as an ordinary message.
#[tokio::test]
async fn a_question_expires_with_the_process_that_asked_it() {
    let [call, request] = question_lines();
    let harness = Harness::ordinary_with(&format!(
        r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  printf '%s\n' '{call}'
  printf '%s\n' '{request}'
  await_go
  rm -f "$GO"
  exit 9
done
"#
    ))
    .await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("pick for me");
    until("the question", || harness.state().question.is_some()).await;
    harness.release();

    until("the question to expire", || {
        harness.registry.host.questions().get("req-q1").map(|stored| stored.state) == Some(QuestionState::Expired)
    })
    .await;
    until("the resume", || harness.spawns().len() == 1).await;
    assert_eq!(harness.state().question, None, "the replacement is asking nothing");
    assert!(matches!(
        harness.registry.answer_question(SESSION, "req-q1", Reply::Dismiss, None).await,
        AnswerOutcome::Unknown
    ));
}

/// The history a server restart finds when it went down with a question
/// open: the turn's prompt, the call, the request — and nothing since.
fn history_with_an_open_question() -> String {
    let [call, request] = question_lines();
    [
        format!(r#"printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'"#),
        format!(r#"printf '%s\n' '{call}'"#),
        format!(r#"printf '%s\n' '{request}'"#),
    ]
    .join("\n")
}

/// A restart does not lose a question: the process is still blocked on it
/// behind the shim, the replay finds it open — and opens the turn it blocks
/// without asking the transcript, which would call it between turns — and
/// it is answerable again.
#[tokio::test]
async fn a_question_outlives_a_server_restart() {
    let harness = Harness::start_attached(
        &history_with_an_open_question(),
        &asks_a_question(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
        Witness::None,
    )
    .await;
    until("the question to be held again", || harness.state().question.is_some()).await;
    assert_eq!(harness.state().phase, Phase::Working, "the turn it blocks is in flight");
    assert_eq!(
        harness.registry.host.questions().get("req-q1").map(|stored| stored.state),
        Some(QuestionState::Pending)
    );
    let outcome = harness
        .registry
        .answer_question(SESSION, "req-q1", Reply::Answer(blue_with_cheese_and_olives()), None)
        .await;
    assert!(matches!(outcome, AnswerOutcome::Delivered(_)), "{outcome:?}");
    until("the turn to end", || harness.state().phase == Phase::Idle).await;
    assert!(response_to(&harness.seen(), "req-q1").is_some());
}

/// The window the record exists for: the last server moved the record to
/// `answered` and died before the answer reached claude's stdin. The replay
/// still sees the question waiting, so the stored answer is written again —
/// safely, because claude ignores a response it has already had.
#[tokio::test]
async fn an_answer_the_last_server_never_wrote_is_written_after_the_restart() {
    let home = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(home.path().to_path_buf());
    InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE)
        .expect("valid name")
        .create()
        .expect("instance directories");
    let store = questions::Questions::new(droppen_instance::db::at(paths.instance_db()));
    let now = questions::now_ms();
    store.open("req-q1", SESSION, Some("toolu_q1"), &colours_and_toppings(), now);
    let delivery = questions::allow(&colours_and_toppings(), &blue_with_cheese_and_olives());
    store
        .resolve(
            "req-q1",
            &[QuestionState::Pending],
            QuestionState::Answered,
            Some(&blue_with_cheese_and_olives()),
            Some(&delivery),
            Some("phone"),
            now,
        )
        .expect("answered before the restart");
    // A question the record holds open and the history does not: settled
    // while nobody was listening, and expired by the replay.
    store.open("req-gone", SESSION, None, &colours_and_toppings(), now);

    let harness = Harness::build_in(
        home,
        &history_with_an_open_question(),
        &asks_a_question(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
        NEVER_SETTLES,
        Adoption {
            attach: Some(Witness::None),
            ..Adoption::default()
        },
    )
    .await;
    until("the stored answer to be written", || response_to(&harness.seen(), "req-q1").is_some()).await;
    let line = response_to(&harness.seen(), "req-q1").expect("written");
    assert_eq!(line["response"]["response"], delivery, "the very answer the user gave");
    // Whole, because the answer went through the database on the way:
    // what comes back out has to be the bytes a live answer is.
    assert_eq!(
        harness.seen().into_iter().find(|line| line.contains("control_response")),
        Some(
            [
                r#"{"type":"control_response","response":{"subtype":"success","request_id":"req-q1","response":{"behavior":"allow","updatedInput":{"questions":"#,
                COLOURS_AND_TOPPINGS,
                r#","answers":{"Which colour do you prefer?":"Blue","Which toppings?":"Cheese, Olives"}}}}}"#,
            ]
            .concat()
        )
    );
    until("the turn to end", || harness.state().phase == Phase::Idle).await;
    assert_eq!(harness.state().question, None, "nothing is left open");
    assert_eq!(
        harness.registry.host.questions().get("req-q1").map(|stored| stored.state),
        Some(QuestionState::Answered)
    );
    assert_eq!(
        harness.registry.host.questions().get("req-gone").map(|stored| stored.state),
        Some(QuestionState::Expired)
    );
}

/// A home with this session's question already on record, as the last
/// server left it — the half of a restart the history cannot show.
fn home_with_a_question(state: QuestionState) -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    let paths = Paths::new(home.path().to_path_buf());
    InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE)
        .expect("valid name")
        .create()
        .expect("instance directories");
    let store = questions::Questions::new(droppen_instance::db::at(paths.instance_db()));
    let now = questions::now_ms();
    store.open("req-q1", SESSION, Some("toolu_q1"), &colours_and_toppings(), now);
    if state != QuestionState::Pending {
        assert!(store.close("req-q1", state, now), "on record as {state:?}");
    }
    home
}

/// The shim's ring is finite, and a CLI blocked on a question can go on
/// publishing housekeeping until the question itself has been lapped out of
/// it. The replay then never sees the question; the history after the gap
/// looks like nothing is happening, and the transcript — ending on the
/// `AskUserQuestion` call — agrees. Only the record still knows: pending,
/// so nobody answered it, so claude is still waiting. It is held open, the
/// turn with it, and a message sent meanwhile waits behind the answer
/// instead of going into a CLI that is blocked.
#[tokio::test]
async fn a_question_lost_in_a_replay_gap_is_still_held_open() {
    let [call, request] = question_lines();
    // 2000 lines of 61 bytes: more than a pipe holds (64 KiB) on top of the
    // 4 KiB ring. The shell cannot finish writing them, and so cannot touch
    // the marker the harness attaches after, until the shim has read all
    // but one pipe's worth, by which point the question has left the ring.
    // With fewer, the attach could come before the shim had read past the
    // question, and the replay would hand it over whole.
    let history = [
        format!(r#"printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'"#),
        format!(r#"printf '%s\n' '{call}'"#),
        format!(r#"printf '%s\n' '{request}'"#),
        r#"i=0; while [ "$i" -lt 2000 ]; do printf '%s\n' '{"type":"system","subtype":"commands_changed","commands":[]}'; i=$((i + 1)); done"#
            .to_string(),
    ]
    .join("\n");
    let harness = Harness::build_in(
        home_with_a_question(QuestionState::Pending),
        &history,
        &asks_a_question(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
        NEVER_SETTLES,
        Adoption {
            attach: Some(Witness::None),
            ring: Some(4096),
            transcript: Some(
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_q1","name":"AskUserQuestion","input":{}}]}}"#
                    .to_string(),
            ),
            ..Adoption::default()
        },
    )
    .await;
    until("the question to be held open across the gap", || harness.state().question.is_some()).await;
    // The recording is written by a task of its own, so the gap can reach
    // the file after the driver has already acted on it.
    until("the recording to show the history lost its start", || {
        harness.recording().iter().any(|event| matches!(event, crate::record::Event::Gap { .. }))
    })
    .await;
    assert_eq!(harness.state().phase, Phase::Working, "the turn the question blocks is open");
    assert_eq!(
        harness.registry.host.questions().get("req-q1").map(|stored| stored.state),
        Some(QuestionState::Pending)
    );

    harness.send("sent while it waits");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(harness.seen().is_empty(), "nothing is written into a blocked CLI: {:?}", harness.seen());
    assert_eq!(harness.state().queued, 1);

    let outcome = harness
        .registry
        .answer_question(SESSION, "req-q1", Reply::Answer(blue_with_cheese_and_olives()), None)
        .await;
    assert!(matches!(outcome, AnswerOutcome::Delivered(_)), "{outcome:?}");
    until("the message to go in once the turn is over", || {
        harness.seen().iter().any(|line| line.contains("sent while it waits"))
    })
    .await;
    assert!(response_to(&harness.seen(), "req-q1").is_some());
}

/// A control request this server has no handler for, left unanswered in
/// the history — the last server died before declining it — is declined
/// again when the replay ends, so the CLI is not left waiting on it.
#[tokio::test]
async fn a_control_request_nobody_answered_is_declined_after_a_restart() {
    let history = [
        format!(r#"printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'"#),
        r#"printf '%s\n' '{"type":"control_request","request_id":"hook-1","request":{"subtype":"hook_callback","callback_id":"x"}}'"#.to_string(),
    ]
    .join("\n");
    let harness = Harness::start_attached(
        &history,
        r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
done
"#,
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
        Witness::None,
    )
    .await;
    until("the request to be declined", || response_to(&harness.seen(), "hook-1").is_some()).await;
    let line = response_to(&harness.seen(), "hook-1").expect("declined");
    assert_eq!(line["response"]["subtype"], "error");
    assert!(line["response"]["error"].as_str().is_some_and(|error| error.contains("Droppen")));
}

/// A stop moves the record to `cancelled` before the interrupt is written.
/// A server that dies between the two leaves claude still waiting on the
/// question; the restart must finish the stop — not dismiss the question,
/// which would let the turn carry on against what the user asked for.
#[tokio::test]
async fn a_stop_that_never_reached_claude_is_made_again_after_a_restart() {
    let harness = Harness::build_in(
        home_with_a_question(QuestionState::Cancelled),
        &history_with_an_open_question(),
        &asks_a_question(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        // No rung in the test's lifetime: the stop under test is the
        // request claude answers. A rung that fired first, on a slow
        // machine, would signal claude before it had read the request.
        Duration::from_secs(3600),
        NEVER_SETTLES,
        Adoption {
            attach: Some(Witness::None),
            ..Adoption::default()
        },
    )
    .await;
    until("the turn to be stopped", || {
        harness.state().phase == Phase::Idle && harness.injection().is_some_and(|run| run.interrupted)
    })
    .await;
    assert!(
        harness
            .seen()
            .iter()
            .any(|line| line.contains("control_request") && line.contains("\"subtype\":\"interrupt\"")),
        "the stop was made again: {:?}",
        harness.seen()
    );
    assert!(
        response_to(&harness.seen(), "req-q1").is_none(),
        "and the question was not answered instead"
    );
    assert_eq!(harness.state().question, None);
    assert_eq!(
        harness.registry.host.questions().get("req-q1").map(|stored| stored.state),
        Some(QuestionState::Cancelled)
    );
}
