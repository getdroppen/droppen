//! Unit and harness tests for the Claude Code resident driver.

mod background_and_recovery;
mod question_tests;
mod rotation_tests;
mod state_machine;

use super::*;
use super::test_harness::*;
use background_and_recovery::{a_settings_change, die_when_released, history_with_an_orphan_task, long_ago};
use std::path::Path;

use droppen_instance::instance::InstanceDirs;
use droppen_instance::paths::Paths;

// ------------------------------------------------------------ pure parts

/// What a test's tracing subscriber wrote.
#[derive(Clone, Default)]
pub(super) struct Logs(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Logs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Logs {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// A send that waits more than two seconds for its resident to journal
/// the message is logged — when it passes the mark, and with the total
/// when it ends — so a slow acknowledgement can be told apart from a slow
/// network in production. A quick one is not; and the wait itself is the
/// one it always was (it still ends only when the driver says so).
#[tokio::test(start_paused = true)]
async fn a_send_that_waits_long_for_the_journal_is_logged_and_a_quick_one_is_not() {
    let logs = Logs::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt().with_writer(move || writer.clone()).with_ansi(false).finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let (tx, rx) = tokio::sync::oneshot::channel();
    tx.send(()).unwrap();
    assert!(wait_recorded("s-quick", false, rx).await.is_ok());
    assert_eq!(logs.text(), "", "a quick journal is not logged");

    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(3_500)).await;
        let _ = tx.send(());
    });
    let started = tokio::time::Instant::now();
    assert!(wait_recorded("s-slow", true, rx).await.is_ok());
    assert_eq!(started.elapsed(), Duration::from_millis(3_500), "it waited for the driver, as before");
    let text = logs.text();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(
        lines[0].contains("still waiting") && lines[0].contains("session=\"s-slow\"") && lines[0].contains("waited_ms=2000"),
        "{text}"
    );
    assert!(
        lines[1].contains("waited_ms=3500") && lines[1].contains("recorded=true") && lines[1].contains("queued=true"),
        "{text}"
    );

    // A driver that ends without recording it: the caller still hears so.
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        drop(tx);
    });
    assert!(wait_recorded("s-gone", false, rx).await.is_err());
    assert!(logs.text().contains("recorded=false"));
}

/// Editing an account's credential has to reach the process already
/// running under it.
///
/// The bug: an env account whose `ANTHROPIC_BASE_URL` had one path segment
/// too many was fixed, every new session worked, and the session the user
/// was sitting in went on failing — because the resident had been started
/// with the old block, and the account id, which was all a fingerprint
/// knew, had not changed.
#[test]
fn the_same_account_with_a_different_credential_is_a_different_process() {
    let under = |credential: &str| Fingerprint {
        account: Some("acc-1".into()),
        credential: Some(credential.into()),
        ..Default::default()
    };
    let live = under("hash-before-the-fix");
    assert!(live.satisfied_by(&live), "the same credential is the same login");
    assert!(
        !under("hash-after-the-fix").satisfied_by(&live),
        "a re-authorized account, or an edited block, needs its own process"
    );
    assert!(
        under("hash-after-the-fix").differences(&live).contains("credentials"),
        "and the report says which of the two it is: {}",
        under("hash-after-the-fix").differences(&live)
    );

    // The local login has no stored credential, and its fingerprint is
    // exactly what it always was.
    let local = Fingerprint::default();
    assert_eq!(local.credential, None);
    assert!(local.satisfied_by(&local));
    assert!(!local.satisfied_by(&live), "still not the account's process");
}

/// The question capability is recorded and never compared, in either
/// direction: a resident from before `--permission-prompt-tool` is not
/// restarted for it, and gets it at its next replacement of its own.
#[test]
fn being_able_to_ask_is_never_a_reason_to_restart() {
    let older = Fingerprint {
        model: Some("opus".into()),
        uploads: true,
        questions: false,
        ..Default::default()
    };
    let wants = Fingerprint {
        questions: true,
        ..older.clone()
    };
    assert!(wants.satisfied_by(&older), "a message from this server fits a resident from the last one");
    assert!(older.satisfied_by(&wants), "and the other way round");
    assert!(
        serde_json::from_str::<Fingerprint>(r#"{"model":"opus","uploads":true}"#).is_ok_and(|read| !read.questions),
        "a record written before the field reads as a process without it"
    );
}

#[test]
fn only_a_widened_uploads_directory_is_forgiven_a_mismatch() {
    let live = Fingerprint {
        model: Some("opus".into()),
        uploads: true,
        ..Default::default()
    };
    assert!(
        Fingerprint {
            model: Some("opus".into()),
            uploads: false,
            ..Default::default()
        }
        .satisfied_by(&live),
        "a message with no attachment fits a process that can read the uploads directory"
    );
    assert!(live.satisfied_by(&live), "and one that needs the directory fits the process that has it");
    assert!(
        !Fingerprint {
            model: Some("opus".into()),
            uploads: true,
            ..Default::default()
        }
        .satisfied_by(&Fingerprint {
            model: Some("opus".into()),
            uploads: false,
            ..Default::default()
        }),
        "the other direction is a real mismatch: the flag is not there to be added later"
    );
    for different in [
        Fingerprint {
            model: Some("sonnet".into()),
            uploads: true,
            ..Default::default()
        },
        Fingerprint {
            model: Some("opus".into()),
            effort: Some("high".into()),
            uploads: true,
            ..Default::default()
        },
        Fingerprint {
            model: Some("opus".into()),
            permission: Some("acceptEdits".into()),
            uploads: true,
            ..Default::default()
        },
        Fingerprint {
            model: Some("opus".into()),
            account: Some("other".into()),
            uploads: true,
            ..Default::default()
        },
        Fingerprint {
            model: Some("opus".into()),
            chat: true,
            uploads: true,
            ..Default::default()
        },
    ] {
        assert!(
            !different.satisfied_by(&live),
            "every command-line setting needs its own process: {different:?}"
        );
        assert!(
            different.accepted_by(None),
            "…but a process nobody described is not a process to refuse a message: {different:?}"
        );
    }
}

/// The model the command line asked for and the model the CLI answers with
/// are the same process, and a send may name either.
///
/// The failure this pins is not theoretical: a resident spawned `--model
/// fable` reports itself as `claude-fable-5`, the composer went on sending
/// `fable`, and each send was read as a settings change — eight turns and
/// eight restarts in one recording, each one killing the session's
/// background work and failing whatever was queued behind it.
#[test]
fn the_model_a_spawn_asked_for_and_the_one_the_cli_answers_with_are_one_process() {
    let live = Fingerprint {
        model: Some("fable".into()),
        resolved: Some("claude-fable-5".into()),
        ..Default::default()
    };
    for name in ["fable", "claude-fable-5"] {
        assert!(
            Fingerprint {
                model: Some(name.into()),
                ..Default::default()
            }
            .satisfied_by(&live),
            "{name} names the process that is running"
        );
    }
    assert!(
        !Fingerprint {
            model: Some("sonnet".into()),
            ..Default::default()
        }
        .satisfied_by(&live),
        "and a third name is still a different model"
    );
    // The other side of it: what a message asks for is a name, never a
    // resolution, so a resolved name it happens to carry is not compared.
    assert!(
        Fingerprint {
            model: Some("fable".into()),
            resolved: Some("something else entirely".into()),
            ..Default::default()
        }
        .satisfied_by(&live),
        "the asking side's resolved name means nothing"
    );
}

/// A message that cannot be delivered says which setting is in the way.
///
/// The only thing a re-delivered message can be told — its environment was
/// never journalled, so there is no process to start for it — and "the
/// settings were lost" told the person who sent it nothing they could act
/// on. Every field the comparison refuses on has a name here.
#[test]
fn a_message_that_cannot_be_delivered_names_what_differs() {
    let live = Fingerprint {
        model: Some("haiku".into()),
        resolved: Some("claude-haiku-4-5".into()),
        account: Some("work".into()),
        ..Default::default()
    };
    let differences = |wanted: Fingerprint| wanted.differences(&live);

    assert_eq!(
        differences(Fingerprint {
            model: Some("opus".into()),
            account: Some("work".into()),
            ..Default::default()
        }),
        "model opus rather than haiku (answering as claude-haiku-4-5)",
        "a process with two names is reported under both, or the answer looks like the question"
    );
    assert_eq!(
        differences(Fingerprint {
            model: Some("opus".into()),
            resolved: None,
            effort: Some("high".into()),
            permission: None,
            account: None,
            credential: None,
            chat: true,
            chat_reads: false,
            uploads: true,
            questions: true,
        }),
        "model opus rather than haiku (answering as claude-haiku-4-5), effort high rather than none, \
         the local login rather than the work account, the chat persona, access to the uploads directory",
        "and every setting that needs its own process is named, in one line"
    );
}

/// What a restart makes of a backlog: who still has to be given what.
///
/// The distinction the `steered` flag exists for. An attach re-attaches to
/// the *same process*, and a message steered into it is in that process'
/// stdin — surviving this server entirely, because stdin belongs to the
/// process. Handing it back as an ordinary queued message would deliver it
/// twice, which for a prompt means running its side effects twice.
#[test]
fn an_attach_leaves_a_steered_message_with_the_process_that_already_has_it() {
    let live = Fingerprint::default();
    let queued = |id: &str, text: &str, steered: bool| Queued {
        id: id.to_string(),
        text: text.to_string(),
        images: Vec::new(),
        files: Vec::new(),
        fingerprint: live.clone(),
        steered,
        rotation_attempt: 0,
        continuation: None,
    };

    let inherited = inherited_queue(SESSION, &[queued("q-1", "urgent", true), queued("", "waiting", false)], Some(&live));
    assert_eq!(inherited.delivered_ahead.len(), 1, "the steered one is not re-delivered");
    assert_eq!(
        inherited.delivered_ahead[0].id, "q-1",
        "and it keeps its id, so a handle a client is holding still names it"
    );
    assert_eq!(inherited.queue.len(), 1);
    assert_eq!(inherited.queue[0].text, "waiting");
    assert!(
        !inherited.queue[0].id.is_empty(),
        "a record from before the queue was addressable gets an id rather than being dropped"
    );

    // The prefix rule is unchanged, and applies across both: the queue is
    // answered in order, so a message the running process cannot take takes
    // everything behind it with it.
    let mismatch = Queued {
        id: "q-2".to_string(),
        text: "under another model".to_string(),
        images: Vec::new(),
        files: Vec::new(),
        fingerprint: Fingerprint {
            model: Some("opus".into()),
            ..Default::default()
        },
        steered: false,
        rotation_attempt: 0,
        continuation: None,
    };
    let inherited = inherited_queue(
        SESSION,
        &[queued("q-1", "urgent", true), mismatch.clone(), queued("q-3", "later", false)],
        Some(&live),
    );
    assert_eq!(inherited.delivered_ahead.len(), 1);
    assert!(inherited.queue.is_empty(), "nothing behind the mismatch survives");

    // …and it applies to a fingerprint, not to the absence of one. A record
    // written before the field existed says *unknown*, and unknown is not a
    // process description to judge messages against: the server that
    // accepted them knew what it was accepting, and this one knows less.
    let inherited = inherited_queue(SESSION, &[queued("q-1", "urgent", true), mismatch, queued("q-3", "later", false)], None);
    assert_eq!(inherited.delivered_ahead.len(), 1);
    assert_eq!(
        inherited.queue.len(),
        2,
        "an unrecorded fingerprint vetoes nothing: the whole backlog survives the restart"
    );
}

/// Nothing the CLI can write is allowed to be an error, and only four kinds
/// of line are allowed to mean anything.
#[test]
fn an_unrecognisable_line_is_never_an_error() {
    assert_eq!(classify("not json at all"), Event::Unparseable);
    assert_eq!(classify(""), Event::Unparseable);
    assert_eq!(classify("{"), Event::Unparseable);
    // Valid JSON that is not an object, and objects with no type: neither is
    // a line to act on, and neither is a failure.
    assert_eq!(classify("[1,2,3]"), Event::Other);
    assert_eq!(classify("{\"no\":\"type\"}"), Event::Other);
    // Types this driver does not act on, including ones invented later.
    for line in [
        r#"{"type":"stream_event"}"#,
        r#"{"type":"rate_limit_event"}"#,
        r#"{"type":"a_type_that_does_not_exist_yet"}"#,
    ] {
        assert_eq!(classify(line), Event::Other, "{line}");
    }
    // A `system` line this driver does not act on is still known for what it
    // is, subtypes invented later included, and a task event too malformed
    // to read is one of those rather than an unknown line. The distinction
    // is the replay's: an unknown line may be a turn's output and a `system`
    // line never is.
    for line in [
        r#"{"type":"system","subtype":"task_started"}"#,
        r#"{"type":"system","subtype":"commands_changed","commands":[]}"#,
        r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":50}"#,
        r#"{"type":"system","subtype":"something_new_in_2_2"}"#,
    ] {
        assert_eq!(classify(line), Event::Housekeeping, "{line}");
    }
    assert_eq!(classify(r#"{"type":"control_response"}"#), Event::ControlResponse);
}

/// claude asking *us* something: the two shapes 2.1.295 writes once it has
/// `--permission-prompt-tool stdio`, and the withdrawal an interrupt makes.
#[test]
fn a_permission_request_and_its_withdrawal_are_read_for_what_they_are() {
    let question = classify(
        r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","display_name":"AskUserQuestion","input":{"questions":[{"question":"Which colour do you prefer?","header":"Colour","options":[{"label":"Red","description":"Warm"},{"label":"Blue","description":"Cool"}],"multiSelect":false}]},"tool_use_id":"toolu_1","requires_user_interaction":true}}"#,
    );
    let Event::Permission(request) = question else {
        panic!("a permission request: {question:?}")
    };
    assert!(request.is_question());
    assert_eq!((request.request_id.as_str(), request.tool_use_id.as_deref()), ("req-1", Some("toolu_1")));

    let write = classify(
        r#"{"type":"control_request","request_id":"req-2","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"file_path":"/etc/hosts"},"tool_use_id":"toolu_2"}}"#,
    );
    assert!(matches!(&write, Event::Permission(request) if !request.is_question()), "{write:?}");

    assert_eq!(
        classify(r#"{"type":"control_cancel_request","request_id":"req-1"}"#),
        Event::ControlCancel { request_id: "req-1".into() }
    );
    assert_eq!(
        classify(r#"{"type":"control_request","request_id":"req-3","request":{"subtype":"hook_callback"}}"#),
        Event::ControlRequest {
            request_id: Some("req-3".into())
        },
        "a request this server does not handle is still answered, so it is told apart"
    );
    assert_eq!(classify(r#"{"type":"control_cancel_request"}"#), Event::Other);
}

/// The one field that tells this session's own thread from a subagent's.
///
/// The whole of [`Driver::on_speech`] rests on it, so it is asserted against
/// the shapes a real session writes rather than against a schema: a
/// forwarded child event carries the `parent_tool_use_id` of the `Task` that
/// started it, and the main thread's own events carry `null` or nothing.
#[test]
fn a_forwarded_subagent_line_is_not_the_main_thread_talking() {
    for line in [
        r#"{"type":"assistant","message":{"model":"claude-fable-5"}}"#,
        r#"{"type":"assistant","parent_tool_use_id":null,"message":{}}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result"}]}}"#,
        r#"{"type":"user","parent_tool_use_id":null,"message":{}}"#,
    ] {
        assert_eq!(classify(line), Event::Speech, "{line}");
    }
    for line in [
        r#"{"type":"assistant","parent_tool_use_id":"toolu_01HuJ","message":{"model":"claude-opus-5"}}"#,
        r#"{"type":"user","parent_tool_use_id":"toolu_01HuJ","message":{}}"#,
    ] {
        assert_eq!(classify(line), Event::Other, "{line}");
    }
    assert_eq!(
        classify(r#"{"type":"system","subtype":"task_updated","task_id":"t1"}"#),
        Event::Housekeeping
    );
    // A `system` line is never the main thread working, whatever its parent
    // field says: these go on while the session sits idle. `task_progress`
    // is read for what it says about the task and is still not speech.
    assert!(matches!(
        classify(r#"{"type":"system","subtype":"task_progress","task_id":"t1","parent_tool_use_id":null}"#),
        Event::TaskProgress { .. }
    ));
}

/// Everything a background panel can show about a running task, which the
/// stream reports every few seconds and this server used to drop.
#[test]
fn a_progress_line_fills_in_the_task_it_names() {
    let line = r#"{"type":"system","subtype":"task_progress","task_id":"t1","tool_use_id":"toolu_09",
        "description":"Reading src/app.rs","subagent_type":"general-purpose",
        "usage":{"total_tokens":10205,"tool_uses":1,"duration_ms":3169},"last_tool_name":"Bash"}"#;
    let Event::TaskProgress { progress } = classify(line) else {
        panic!("a task_progress line is its own event");
    };

    let mut tasks = BackgroundTasks::default();
    assert!(!tasks.progress(progress.clone()), "a task nobody announced is not invented here");
    tasks.observe(TaskSeen {
        id: "t1".into(),
        tool_use_id: None,
        description: Some("Sweep the parser".into()),
        task_type: Some("local_agent".into()),
    });
    assert!(tasks.progress(progress.clone()), "the first progress line is news");
    assert!(!tasks.progress(progress), "the same numbers again are not");

    let task = &tasks.wire()[0];
    assert_eq!(task.last_tool_name.as_deref(), Some("Bash"));
    assert_eq!(task.subagent_type.as_deref(), Some("general-purpose"));
    assert_eq!(
        (task.total_tokens.0, task.tool_uses.0, task.duration_ms.0),
        (Some(10205), Some(1), Some(3169))
    );
    assert_eq!(
        task.tool_use_id.0.as_deref(),
        Some("toolu_09"),
        "progress carries the join key a restart's snapshot lacks"
    );
    assert_eq!(
        task.description.0.as_deref(),
        Some("Sweep the parser"),
        "progress describes the tool of the moment, never what the task was asked for"
    );
}

#[test]
fn the_two_lines_that_matter_are_read_with_defaults_rather_than_a_schema() {
    assert_eq!(
        classify(r#"{"type":"system","subtype":"init","session_id":"abc","model":"claude-haiku-4-5-20251001","tools":[]}"#),
        Event::Init {
            session: Some("abc".into()),
            model: Some("claude-haiku-4-5-20251001".into())
        }
    );
    assert_eq!(
        classify(r#"{"type":"system","subtype":"init"}"#),
        Event::Init { session: None, model: None },
        "an init with no id still means the process is listening"
    );
    assert_eq!(
        classify(r#"{"type":"result","subtype":"success","is_error":false,"result":"hi"}"#),
        Event::Result {
            is_error: false,
            subtype: "success".into(),
            detail: Some("hi".into()),
            api_status: None,
        }
    );
    assert_eq!(
        classify(r#"{"type":"result"}"#),
        Event::Result {
            is_error: false,
            subtype: "an error".into(),
            detail: None,
            api_status: None,
        },
        "a result stripped of every field is still the end of the turn"
    );
}

/// The limit log's two claude inputs, as claude 2.1.266 writes them.
///
/// A session limit is a `result` with `is_error: true` and — confusingly —
/// `subtype: success`, carrying the HTTP status beside the words. And the
/// `rate_limit_event` before it says `rejected`, where the one before
/// nearly every other turn says `allowed`: only the refusal is an event.
#[test]
fn a_limit_result_and_a_refused_rate_limit_event_are_read_for_the_log() {
    assert_eq!(
        classify(
            r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":429,"terminal_reason":"api_error","result":"You've hit your session limit · resets 2:30am (Asia/Tokyo)"}"#
        ),
        Event::Result {
            is_error: true,
            subtype: "success".into(),
            detail: Some("You've hit your session limit · resets 2:30am (Asia/Tokyo)".into()),
            api_status: Some(429),
        }
    );

    let rejected = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1789534200,"rateLimitType":"five_hour"},"uuid":"u","session_id":"s"}"#;
    match classify(rejected) {
        Event::RateLimit { info } => {
            assert_eq!(info["rateLimitType"], "five_hour");
            assert_eq!(info["resetsAt"], 1789534200);
        }
        other => panic!("a refusal is its own event, got {other:?}"),
    }
    for status in ["allowed", "allowed_warning"] {
        let line = format!(
            r#"{{"type":"rate_limit_event","rate_limit_info":{{"status":"{status}","resetsAt":1790157600,"rateLimitType":"five_hour"}}}}"#
        );
        assert_eq!(classify(&line), Event::Other, "a reading is not an event: {status}");
    }
}

/// Verbatim: the `allowed` event claude writes before nearly every turn.
const ALLOWED_WITH_WINDOWS: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"isUsingOverage":false,"overageDisabledReason":"org_level_disabled","overageStatus":"rejected","rateLimitType":"five_hour","resetsAt":1790436000,"status":"allowed","unifiedWindows":{"five_hour":{"resetsAt":1790436000,"utilization":0.19},"seven_day":{"resetsAt":1790460000,"utilization":0.46}}}}"#;

/// An `allowed` event is not a refusal, but its windows are a reading, so
/// it is told apart from the lines that say nothing — and a refusal carries
/// its windows through to the same place.
#[test]
fn a_rate_limit_event_with_windows_is_a_usage_reading_whatever_its_status() {
    match classify(ALLOWED_WITH_WINDOWS) {
        Event::UsageReading { info } => {
            assert_eq!(info["status"], "allowed");
            let windows = info.windows_from_rate_limit_info();
            assert_eq!(windows.len(), 2);
            assert_eq!(windows[0].label, "5h");
            assert_eq!(windows[0].used_percent, 19.0);
        }
        other => panic!("an allowed event with windows is a reading, got {other:?}"),
    }
    let rejected = r#"{"type":"rate_limit_event","rate_limit_info":{"isUsingOverage":false,"overageDisabledReason":"org_level_disabled","overageStatus":"rejected","rateLimitType":"seven_day_overage_included","resetsAt":1790575200,"status":"rejected","unifiedWindows":{"five_hour":{"resetsAt":1790436000,"utilization":0.02},"seven_day":{"resetsAt":1790575200,"utilization":0.82},"seven_day_overage_included":{"resetsAt":1790575200,"utilization":1}}}}"#;
    match classify(rejected) {
        Event::RateLimit { info } => {
            let labels: Vec<String> = info.windows_from_rate_limit_info()
                .into_iter()
                .map(|w| w.label)
                .collect();
            assert_eq!(
                labels,
                ["5h", "weekly", "weekly (Fable)"],
                "the refusal is still recorded as one, windows and all"
            );
        }
        other => panic!("a refusal stays a refusal, got {other:?}"),
    }
    // No windows, nothing to file.
    assert_eq!(
        classify(r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","unifiedWindows":null}}"#),
        Event::Other
    );
}

/// Which login a session's reading is filed under, from the environment the
/// server really builds for each kind of spawn.
#[test]
fn a_usage_reading_goes_to_the_login_the_process_runs_on() {
    use crate::quota::session_login;
    use droppen_accounts::quota::UsageLogin;
    use droppen_instance::paths::ClaudeRoot;

    let local = droppen_base::process_env::claude_env_bare();
    let managed = droppen_base::process_env::claude_env([("CLAUDE_CONFIG_DIR", "/data/instances/default/claude")]);
    assert_eq!(recipe_root(&local), ClaudeRoot::Local, "a local-login spawn names no config dir");
    assert_eq!(recipe_root(&managed), ClaudeRoot::Managed);

    let id = "0b6f7a7e-2f7a-4d8e-9d0c-1a2b3c4d5e6f";
    // An account session: the account, in whichever root.
    assert_eq!(session_login(Some(id), None), Some(UsageLogin::Account(id.into())));
    // No account, the user's own `~/.claude`: the local login.
    assert_eq!(session_login(None, Some(recipe_root(&local))), Some(UsageLogin::Local));
    // No account, the managed root: some other login — skipped.
    assert_eq!(session_login(None, Some(recipe_root(&managed))), None);
}

#[test]
fn a_failure_tail_keeps_the_last_lines_and_nothing_longer() {
    let stderr = (1..=10).map(|n| format!("line {n}\n")).collect::<String>();
    let tail = last_lines(&stderr, 6);
    assert_eq!(tail.lines().count(), 6);
    assert!(tail.starts_with("line 5"));
    assert!(!tail.contains("line 4"));
    assert_eq!(last_lines("only one", 6), "only one");
    assert_eq!(last_lines("", 6), "");
    assert_eq!(clip("short"), "short");
    assert_eq!(clip(&"x".repeat(LOG_LINE_CAP + 10)).chars().count(), LOG_LINE_CAP + 1);
}

// --------------------------------------------------------- the harness

/// The provider registry reads Claude Code's runs for the session list
/// and the boot pass: a resident is in the list's snapshot and in the one
/// its turn's alert carries, and the question it is blocked on outlives
/// the boot pass, while one of a session whose process did not come back
/// expires.
#[tokio::test]
async fn the_registry_reads_claude_code_s_runs_for_the_list_and_the_boot_pass() {
    const HELD: &str = "held-session";
    const GONE: &str = "gone-session";
    let home = tempfile::tempdir().expect("tempdir");
    let minute = Duration::from_secs(60);
    let (injections, claude) = registry(home.path(), minute, minute, minute, minute, minute);
    let providers = droppen_provider::ProviderRegistry::new(injections.clone(), |providers, injections| {
        providers.register(Arc::new(ClaudeCode::on(claude.clone(), injections)));
    });
    let (tx, _rx) = mpsc::unbounded_channel();
    let shared = Arc::new(Mutex::new(ResidentState {
        phase: Phase::Idle,
        stalled: false,
        queued: 0,
        queued_messages: Vec::new(),
        background: 0,
        tasks: Vec::new(),
        awaiting_background: false,
        turn_started_at: None,
        question: None,
        permission: None,
        model: None,
    }));
    claude.residents.lock().expect("residents").insert(HELD.to_string(), Handle { tx, shared });

    assert!(providers.driver_states().resident(Source::Claude, HELD).is_some(), "in the list's snapshot");
    assert!(injections.host.driver_states_of(&*claude).resident(Source::Claude, HELD).is_some(), "in the alert's");

    let asked = serde_json::json!([{"question": "Which one?", "options": [{"label": "this"}]}]);
    for (request, session) in [("request-held", HELD), ("request-gone", GONE)] {
        assert!(injections.host.questions().open(request, session, None, &asked, questions::now_ms()));
    }
    providers.adopt_all().await;
    let state = |session| injections.host.questions().for_session(session, questions::now_ms())[0].state;
    assert_eq!(state(HELD), droppen_sessions::delivery::QuestionState::Pending, "its process is still there");
    assert_eq!(state(GONE), droppen_sessions::delivery::QuestionState::Expired);
}

/// The CLI that **folds** a mid-turn message into the turn it interrupted.
///
/// One of the two things claude 2.1.231 really does with a `user` line
/// written while a turn is running, and the one that used to wedge a
/// session: the message is injected at the next tool boundary, the model
/// answers both prompts in one reply, and the *original* turn's `result` is
/// the only one that ever goes out. No `init`, no second `result`, no echo
/// of the injected text — nothing on stdout distinguishes it at all.
///
/// Modelled by reading the steered line before the turn is released rather
/// than by peeking at the pipe, which no portable `sh` can do: the second
/// `read` blocks until the steer arrives, and the one `result` after it
/// covers both messages. Everything after that turn is ordinary.
fn folds_the_steered_message() -> &'static str {
    r#"
IFS= read -r first
printf '%s\n' "$first" >> "$LOG"
IFS= read -r second
printf '%s\n' "$second" >> "$LOG"
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

/// The CLI that **buffers** a mid-turn message and runs it as its own turn.
///
/// The other half of the same CLI, and the half [`preamble`] cannot show:
/// every turn after the first announces itself with a `system:init` of its
/// own before it does anything, which is precisely the evidence
/// [`Driver::settle_delivered`] waits for. The turn is then held, so a
/// settle window that fired anyway would be caught: the message spends far
/// longer than the window running.
fn defers_the_steered_message() -> String {
    format!(
        r#"
first=1
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
*control_request*) continue ;;
  esac
  if [ "$first" = 0 ]; then
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'
  fi
  first=0
  await_go
  rm -f "$GO"
  printf '%s\n' '{{"type":"assistant","message":{{}}}}'
  printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}'
done
"#
    )
}

/// Whether a recording holds a lifecycle line for this stage.
fn staged(events: &[crate::record::Event], stage: crate::record::Stage) -> bool {
    events
        .iter()
        .any(|event| matches!(event, crate::record::Event::Lifecycle { event, .. } if *event == stage))
}

/// A control request this driver wrote, with the uuid in its
/// `request_id` replaced by `<uuid>`: the one part of it that is fresh
/// every time. Checked to be a lowercase, hyphenated v4 uuid right after
/// `prefix`, so the mask hides nothing else.
fn masked_request_id(line: &str, prefix: &str) -> String {
    let value: Value = serde_json::from_str(line).expect("one JSON object");
    let id = value["request_id"].as_str().expect("a request id");
    let fresh = id.strip_prefix(prefix).unwrap_or_else(|| panic!("{id} starts with {prefix}"));
    let uuid = uuid::Uuid::parse_str(fresh).expect("a uuid");
    assert_eq!(fresh, uuid.hyphenated().to_string(), "lowercase and hyphenated");
    assert_eq!(uuid.get_version_num(), 4, "a random uuid");
    line.replacen(fresh, "<uuid>", 1)
}

// ------------------------------------------------------- durability

/// A message sent behind a running turn is in the journal by the time its
/// send is answered.
///
/// The send route says `200` the moment [`ClaudeCodeRuns::deliver`]
/// returns, and a server killed the instant after hands the next one
/// nothing but the journal. So the journal is read here straight after the
/// call returns, without polling: polling would wait for the driver and
/// hide the window this is about.
///
/// On the single-threaded test runtime on purpose. Nothing else runs until
/// this task yields, so a `deliver` that posted to the driver's inbox and
/// returned — which is what it used to do — finds the journal without the
/// message every time, rather than whenever a busy machine lets the race go
/// the wrong way.
#[tokio::test]
async fn a_message_sent_behind_a_turn_is_on_disk_before_its_send_is_answered() {
    let harness = Harness::ordinary().await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("first");
    until("the first to be taken into a turn", || {
        harness.state().phase == Phase::Working && harness.record().payload.turn_at.is_some()
    })
    .await;

    let delivery = harness
        .registry
        .deliver(harness.providers.gate(), SESSION, "second", Vec::new(), Vec::new(), harness.recipe(Fingerprint::default()), None)
        .await
        .expect("the running resident takes it");
    let record = harness.record();
    assert!(delivery.queued, "it waits behind the turn in flight");
    assert_eq!(
        record.payload.queued.iter().map(|queued| queued.text.as_str()).collect::<Vec<_>>(),
        vec!["second"],
        "the accepted message is on disk when the send is answered, not merely in the driver's inbox: {record:?}"
    );
    assert!(record.payload.turn_at.is_some(), "behind the turn that is still in flight: {record:?}");

    // And it is still a queue that moves: the turn ends, and it goes in.
    harness.release();
    until("the second to reach claude", || harness.seen().iter().any(|line| line.contains("second"))).await;
    harness.release();
}

// ------------------------------------------------------ receipts

/// A named message's receipt follows it: queued while it waits behind a
/// turn, sent the moment a turn takes it — and the message sent into an
/// idle session is sent at once.
#[tokio::test]
async fn a_named_messages_receipt_follows_it_from_the_queue_into_its_turn() {
    let harness = Harness::ordinary().await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    let first = harness.named("first");
    until("the first to be taken into a turn", || {
        harness.receipt(&first) == Some(ReceiptStatus::Sent)
    })
    .await;
    assert_eq!(harness.state().phase, Phase::Working);

    let second = harness.named("second");
    until("the second to wait", || harness.state().queued == 1).await;
    assert_eq!(harness.receipt(&second), Some(ReceiptStatus::Queued), "behind a turn, it is only queued");

    harness.release();
    until("the second to be taken into its turn", || {
        harness.receipt(&second) == Some(ReceiptStatus::Sent)
    })
    .await;
    assert_eq!(harness.receipt(&first), Some(ReceiptStatus::Sent), "and the first stays sent");
    harness.release();
}

/// Withdrawn is failed — the server dropped it unsent — and steered is
/// sent: it is in claude's stdin, whichever turn ends up answering it.
#[tokio::test]
async fn a_named_message_withdrawn_is_failed_and_one_steered_is_sent() {
    let harness = Harness::ordinary().await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("busy");
    until("the turn", || harness.state().phase == Phase::Working).await;

    let dropped = harness.named("withdrawn");
    let steered = harness.named("steered");
    until("both to wait", || harness.state().queued == 2).await;
    assert_eq!(harness.withdraw(&dropped).await, QueueOutcome::Done);
    assert_eq!(harness.receipt(&dropped), Some(ReceiptStatus::Failed));
    assert_eq!(harness.steer(&steered).await, QueueOutcome::Done);
    assert_eq!(harness.receipt(&steered), Some(ReceiptStatus::Sent));
    harness.release();
}

/// The queue a session that cannot be brought back leaves behind is
/// dropped — "N queued message(s) were not delivered" — and a named
/// message in it says failed rather than queued for ever.
#[tokio::test]
async fn a_named_message_left_in_a_dropped_queue_is_failed() {
    let harness = Harness::ordinary_with(die_when_released()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.refuse_to_start();
    harness.send("doomed");
    until("the turn", || harness.state().phase == Phase::Working).await;
    // More than the recovery ladder has rungs: each replacement takes the
    // head of the queue into a turn of its own before it dies, so the last
    // of these is never written anywhere.
    for n in 0..4 {
        harness.send(&format!("filler {n}"));
    }
    let left = harness.named("left behind");
    until("the queue", || harness.state().queued == 5).await;
    assert_eq!(harness.receipt(&left), Some(ReceiptStatus::Queued));
    harness.release();

    until("the breaker to trip", || harness.driver.is_finished()).await;
    assert_eq!(harness.receipt(&left), Some(ReceiptStatus::Failed));
    let error = harness.injection().expect("a run").error.0.expect("a reason");
    assert!(error.contains("were not delivered"), "{error}");
}

// ------------------------------------------------------ the recording

/// One ordinary turn, as [`crate::record`] writes it down.
///
/// The sequence is the contract replay depends on: what went in, what came
/// back, where the turn opened and closed, and — the part no stdio stream
/// carries — what the CLI appended to its transcript files while it worked,
/// a subagent that appeared mid-turn included.
#[tokio::test]
async fn a_session_records_everything_the_replay_will_need() {
    use crate::record::Event;

    let harness = Harness::ordinary().await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    // A transcript, as a real claude would be writing one. The fake cannot:
    // it is a shell script that speaks stream-json, and the two halves of a
    // session are written by the CLI independently.
    let transcript = harness.transcript();
    std::fs::write(&transcript, "{\"type\":\"user\",\"n\":1}\n").expect("transcript");

    harness.send("first");
    until("the turn", || harness.state().phase == Phase::Working).await;
    std::fs::write(&transcript, "{\"type\":\"user\",\"n\":1}\n{\"type\":\"assistant\",\"n\":2}\n").expect("transcript");
    // …and a subagent that only exists because this turn started one.
    let agent = crate::sessions::transcript::subagent_path(&transcript, "a1");
    std::fs::create_dir_all(agent.parent().expect("subagents")).expect("mkdir");
    std::fs::write(&agent, "{\"type\":\"assistant\",\"isSidechain\":true}\n").expect("agent transcript");

    harness.release();
    until("the result", || harness.state().phase == Phase::Idle).await;
    // Wait on the recording file's own completion signals rather than the
    // driver's phase: the recorder writes asynchronously on a background task,
    // and the transcript tailer can flush both files before `release()` while
    // `Stdout(result)` and `Stage::Result` are still queued for the writer.
    until("the recording to include the result and transcripts", || {
        let events = harness.recording();
        staged(&events, crate::record::Stage::Result)
            && events
                .iter()
                .any(|event| matches!(event, Event::Transcript { file, .. } if file == "agent:a1"))
            && events
                .iter()
                .filter(|event| matches!(event, Event::Transcript { file, .. } if file == "main"))
                .count()
                == 2
    })
    .await;

    let events = harness.recording();
    // The prompt, exactly as claude read it.
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Stdin { line } if line.contains("\"first\""))),
        "the message that opened the turn is in the recording: {events:?}"
    );
    // And what came back, including the preamble junk a tolerant parser
    // steps over — a recording is what was *said*, not what was understood.
    let stdout: Vec<&String> = events
        .iter()
        .filter_map(|event| match event {
            Event::Stdout { line, replayed: false } => Some(line),
            _ => None,
        })
        .collect();
    assert!(stdout.iter().any(|line| line.contains("\"subtype\":\"init\"")), "{stdout:?}");
    assert!(stdout.iter().any(|line| line.contains("this line is not json")), "{stdout:?}");
    assert!(stdout.iter().any(|line| line.contains("\"type\":\"result\"")), "{stdout:?}");

    assert!(staged(&events, crate::record::Stage::TurnOpen));
    assert!(staged(&events, crate::record::Stage::Result));
    // Order, not just presence: a turn opens before it ends, and the prompt
    // goes in before the answer comes back.
    let position = |wanted: fn(&Event) -> bool| events.iter().position(wanted).expect("an expected event");
    let opened = position(|event| matches!(event, Event::Lifecycle { event, .. } if *event == crate::record::Stage::TurnOpen));
    let ended = position(|event| matches!(event, Event::Lifecycle { event, .. } if *event == crate::record::Stage::Result));
    let prompt = position(|event| matches!(event, Event::Stdin { line } if line.contains("\"first\"")));
    assert!(prompt < opened || opened < ended, "the turn's line order is wrong: {events:?}");
    assert!(opened < ended, "a turn ends after it opens: {events:?}");

    // The transcript lines are the CLI's own, verbatim and in order.
    let main: Vec<&String> = events
        .iter()
        .filter_map(|event| match event {
            Event::Transcript { file, line, .. } if file == "main" => Some(line),
            _ => None,
        })
        .collect();
    assert_eq!(main, ["{\"type\":\"user\",\"n\":1}", "{\"type\":\"assistant\",\"n\":2}"], "{main:?}");
}

// -------------------------------------------------------------- drafts

/// What the session's draft holds right now.
pub(super) fn draft_of(harness: &Harness) -> droppen_provider::drafts::Draft {
    harness
        .providers()
        .draft_watch(Source::Claude, SESSION)
        .expect("a claude session has a draft")
        .borrow()
        .clone()
}

/// A think in flight is counted while the turn is held — one count, moved
/// by each `thinking_tokens`, which a subagent's line in between does not
/// end — and the `result` leaves nothing counted.
#[tokio::test]
async fn a_think_is_counted_until_the_turn_moves_on() {
    let harness = Harness::ordinary_with(&thinks_until_released()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    let before = draft_of(&harness).v;
    harness.send("think about it");
    until("the think to be counted", || draft_of(&harness).thinking == Some(42)).await;
    let draft = draft_of(&harness);
    assert!(draft.open);
    assert_eq!(draft.v, before + 2, "started at 40 and moved to 42, never ended in between: {draft:?}");

    harness.release();
    until("the turn to end", || {
        harness.state().phase == Phase::Idle && harness.injection().is_some_and(|run| run.state == InjectionRunState::Ok)
    })
    .await;
    let closed = draft_of(&harness);
    assert!(closed.is_empty() && !closed.open, "{closed:?}");
    assert_eq!(closed.turn, draft.turn);
}

/// A process that dies mid-think takes its count with it at once — there
/// is no `result` to end it.
#[tokio::test]
async fn a_dead_process_takes_its_count_with_it() {
    let body = r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  printf '%s\n' '{"type":"system","subtype":"thinking_tokens","estimated_tokens":7,"estimated_tokens_delta":7}'
  await_go
  rm -f "$GO"
  exit 9
done
"#;
    let harness = Harness::ordinary_with(body).await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("doomed");
    until("the think to be counted", || draft_of(&harness).thinking == Some(7)).await;
    harness.release();
    until("the resume", || harness.spawns().len() == 1).await;
    until("the count to be cleared", || draft_of(&harness).is_empty()).await;
    assert!(!draft_of(&harness).open);
}

// ------------------------------------------------ the gate and the claim

/// A new resident starts under the spawn gate and with its claim, and both
/// are held across the spawn and the map insert: until it is in the map, a
/// second send could start a second claude. The gate goes with the send
/// that started it and the claim stays with the driver. A message to the
/// resident starts nothing, and lets the gate go before it waits for the
/// journal.
#[tokio::test]
async fn a_claude_start_holds_the_gate_and_its_claim_until_the_resident_is_in_the_map() {
    const OTHER: &str = "44444444-4444-4444-8444-444444444444";
    let harness = Harness::ordinary().await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.plan_claude(&replacement_claude(OTHER, FORKED));
    let registry = &harness.registry;
    let gate = harness.providers.gate();

    registry
        .deliver(gate, OTHER, "first", Vec::new(), Vec::new(), harness.recipe(Fingerprint::default()), None)
        .await
        .expect("a resident starts");
    assert_eq!(
        registry.host.steps(),
        [step("claude started", true, Some(true)), step("resident mapped", true, Some(true))]
    );
    assert!(!registry.host.gate_held(), "the send that started it let the gate go");
    assert!(registry.host.claim_held(Source::Claude, OTHER), "the driver holds the claim");

    registry
        .deliver(gate, OTHER, "second", Vec::new(), Vec::new(), harness.recipe(Fingerprint::default()), None)
        .await
        .expect("the resident takes it");
    assert_eq!(registry.host.steps()[2..], [step("posted to the resident", false, Some(true))]);
    assert!(!registry.host.gate_held());
}

/// A resident's respawn takes no gate: its entry in the map already keeps a
/// second start away, and the claim it started with stays on. The claim
/// goes when the driver ends, after its entry and its journal row.
#[tokio::test]
async fn a_claude_respawn_takes_no_gate_and_the_claim_goes_when_the_driver_ends() {
    const OTHER: &str = "55555555-5555-4555-8555-555555555555";
    let harness = Harness::ordinary().await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.plan_claude(&replacement_claude(OTHER, FORKED));
    let registry = &harness.registry;
    let gate = harness.providers.gate();
    registry
        .deliver(gate, OTHER, "first", Vec::new(), Vec::new(), harness.recipe(Fingerprint::default()), None)
        .await
        .expect("a resident starts");

    // A message for another model: the driver replaces the process.
    let other_model = Fingerprint {
        model: Some("another-model".to_string()),
        ..Fingerprint::default()
    };
    registry
        .deliver(gate, OTHER, "second", Vec::new(), Vec::new(), harness.recipe(other_model), None)
        .await
        .expect("taken");
    until("the replacement to answer", || harness.after().iter().any(|line| line.contains("second"))).await;
    assert_eq!(registry.host.steps()[3..], [step("claude started", false, Some(true))]);

    // The process goes while idle, so the driver ends.
    until("the replacement to go idle", || {
        registry
            .residents
            .lock()
            .expect("residents")
            .get(OTHER)
            .is_some_and(|handle| handle.state().phase == Phase::Idle)
    })
    .await;
    let pid = registry
        .host.journal
        .read()
        .into_iter()
        .find(|record| record.session_id == OTHER)
        .expect("its record")
        .cli_pid;
    droppen_provider::testing::signal(pid, libc::SIGKILL);
    until("the driver to end", || registry.host.steps().len() == 5).await;
    assert_eq!(registry.host.steps()[4..], [step("resident ended", false, Some(false))]);
    assert!(!registry.residents.lock().expect("residents").contains_key(OTHER));
}

/// A resident that claude answers under another id follows it: the new id
/// is claimed without the gate, and the old one's claim goes after.
#[tokio::test]
async fn a_resident_claude_renames_claims_the_new_id_without_the_gate() {
    const OTHER: &str = "66666666-6666-4666-8666-666666666666";
    const RENAMED: &str = "77777777-7777-4777-8777-777777777777";
    let harness = Harness::ordinary().await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.plan_claude(&replacement_claude(RENAMED, RENAMED));
    let registry = &harness.registry;
    let gate = harness.providers.gate();

    registry
        .deliver(gate, OTHER, "first", Vec::new(), Vec::new(), harness.recipe(Fingerprint::default()), None)
        .await
        .expect("a resident starts");
    until("the rename", || registry.host.steps().len() == 4).await;
    assert_eq!(
        registry.host.steps()[2..],
        [step("adopted: the new id", false, Some(true)), step("adopted: the old id", false, Some(false))]
    );
}

/// The `result` an API error before the stream starts comes back as: the
/// subtype is still `success`, and only the text says otherwise.
///
/// "claude ended the turn with success" over an error reads as a
/// contradiction — and on a gateway that words its context overflow its own
/// way, that error *is* the whole explanation. The detail stands alone.
#[tokio::test]
async fn an_api_error_is_reported_without_calling_it_a_success() {
    let harness = Harness::start(
        r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in *control_request*) continue ;; esac
  printf '%s\n' '{"type":"result","subtype":"success","is_error":true,"result":"API Error: 400 maximum context length is 1048576 tokens"}'
done
"#,
        NEVER_SETTLES,
        NEVER_SETTLES,
        NEVER_SETTLES,
    )
    .await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send("go");
    until("the failure", || {
        harness.injection().is_some_and(|run| run.state == InjectionRunState::Failed)
    })
    .await;

    let error = harness.injection().expect("a run").error.0.expect("a reason");
    assert_eq!(error, "API Error: 400 maximum context length is 1048576 tokens");
    assert!(!error.contains("with success"), "{error}");
}
