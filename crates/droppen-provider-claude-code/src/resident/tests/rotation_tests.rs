//! Account rotation and ahead-of-limit tests.

use super::*;

// ------------------------------------------------------ account rotation

/// The `result` a refused turn ends with, as claude 2.1.266 writes it — the
/// apostrophe in "You've" dropped, because the line has to fit inside a
/// single-quoted shell string.
const SESSION_LIMIT: &str =
    r#"{"type":"result","subtype":"success","is_error":true,"result":"You have hit your session limit","api_error_status":429}"#;

/// The Fable weekly refusal, word for word but for the same apostrophe.
const FABLE_LIMIT: &str = r#"{"type":"result","subtype":"success","is_error":true,"result":"You have reached your Fable limit. Switch to another model, or manage usage credits at claude.ai/settings/usage?from=cc_cli_limit_message, to continue.","api_error_status":429}"#;

/// An ordinary finished turn.
const TURN_OK: &str = r#"{"type":"result","subtype":"success","is_error":false}"#;

/// A claude refused on its first message — the refused `rate_limit_event`
/// for `window` and then `result`, the order every recording shows — that
/// answers every later message normally. `before` is written first, for a
/// background task the refusal will find running. Every turn waits for
/// `$GO`, so a test can queue messages behind it. The counter is not `n`:
/// `await_go` is a shell function, and shell functions share their
/// caller's variables.
fn refused_once(window: &str, resets_at: i64, result: &str, before: &str) -> String {
    format!(
        r#"
turns=0
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in *control_request*) continue ;; esac
  turns=$((turns + 1))
  await_go
  rm -f "$GO"
  if [ "$turns" = 1 ]; then
{before}
printf '%s\n' '{{"type":"rate_limit_event","rate_limit_info":{{"status":"rejected","rateLimitType":"{window}","resetsAt":{resets_at},"overageStatus":"rejected","isUsingOverage":false}}}}'
printf '%s\n' '{result}'
  else
printf '%s\n' '{TURN_OK}'
  fi
done
"#
    )
}

/// The `claude` a rotation's respawn finds: it notes its argv and which
/// token it was given — a fake one, minted by the test — and answers every
/// message with `answer`.
fn rotated_claude(answer: &str) -> String {
    format!(
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$SPAWNS"
printf 'token=%s\n' "${{CLAUDE_CODE_OAUTH_TOKEN:-none}}" >> "$SPAWNS"
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$AFTER"
  case "$line" in
*control_request*) : ;;
*) printf '%s\n' '{answer}' ;;
  esac
done
"#
    )
}

/// One background task, announced the way claude announces them.
fn one_background_task(id: &str, description: &str) -> String {
    format!(
        r#"    printf '%s\n' '{{"type":"system","subtype":"background_tasks_changed","tasks":[{{"task_id":"{id}","task_type":"local_bash","description":"{description}"}}]}}'"#
    )
}

/// The fake token a test account holds. Not a credential of anybody's.
fn fake_token(label: &str) -> String {
    format!("sk-ant-oat01-test-{}", label.replace(['@', ' '], "-"))
}

/// Store `wanted` accounts in `home` — `(label, oauth)`: an OAuth account
/// is in the pool by default, an env one is not — bind the session to the
/// first, and describe a process started under it running `model`.
async fn accounts_in(home: &Path, wanted: &[(&str, bool)], model: Option<&str>) -> (Vec<droppen_accounts::store::Account>, Fingerprint) {
    use droppen_accounts::store::{AccountsStore, Credential};
    use droppen_base::env_block::parse_env_block;
    let store = AccountsStore::new(Arc::new(Paths::new(home.to_path_buf())), logins());
    let mut accounts = Vec::new();
    for (label, oauth) in wanted {
        let credential = if *oauth {
            Credential::Oauth(fake_token(label))
        } else {
            Credential::Env(
                parse_env_block(&format!(
                    "ANTHROPIC_AUTH_TOKEN={}\nANTHROPIC_BASE_URL=https://relay.example",
                    fake_token(label)
                ))
                .expect("block"),
            )
        };
        accounts.push(store.create(label, AccountProvider::Claude, credential).await.expect("account"));
    }
    let first = &accounts[0];
    store
        .bind_session_account(SESSION, Some(&first.id), AccountProvider::Claude)
        .await
        .expect("bind");
    let live = Fingerprint {
        model: model.map(str::to_owned),
        account: Some(first.id.clone()),
        credential: store.credential_hash(&first.id).await,
        ..Default::default()
    };
    (accounts, live)
}

/// The whole of phase 2, on the everyday case: the five-hour window runs
/// out mid-conversation with a message queued behind the turn.
///
/// The queued message must not go to the account that just refused — that
/// is the bug, five rejections in a minute in one real session — and the
/// failed message must not be sent again: it is in the transcript already.
/// What goes to the next account is a continuation, then the queue.
#[tokio::test]
async fn a_session_limit_moves_the_session_to_the_next_pool_account_and_carries_on() {
    let home = tempfile::tempdir().expect("tempdir");
    let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], None).await;
    let (a, b) = (accounts[0].clone(), accounts[1].clone());
    let resets = rest::now_ms() / 1000 + 7200;
    let harness = Harness::rotating(home, &refused_once("five_hour", resets, SESSION_LIMIT, ""), live.clone()).await;
    harness.plan_claude(&rotated_claude(TURN_OK));
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send_with("first", harness.recipe(live.clone()));
    until("the first turn", || harness.state().phase == Phase::Working).await;
    harness.send_with("second", harness.recipe(live.clone()));
    until("the second message to queue", || harness.state().queued == 1).await;
    harness.release();

    until("the continuation and the queued message to reach the new account", || {
        harness.after().len() == 2 && harness.state().phase == Phase::Idle
    })
    .await;
    let seen = harness.seen();
    assert_eq!(seen.len(), 1, "the refusing account got the first message and nothing else: {seen:?}");
    assert!(seen[0].contains("first"));
    let after = harness.after();
    assert!(
        after[0].contains("[Droppen] 这个会话已从账号 a@work 自动切换到 b@team：a@work 的 5 小时额度用完"),
        "the new account is told what happened instead of being sent the prompt again: {}",
        after[0]
    );
    assert!(after[0].contains("请从中断处继续"), "{}", after[0]);
    assert!(
        !after.iter().any(|line| line.contains("\"first\"")),
        "the failed message is not re-sent: {after:?}"
    );
    assert!(after[1].contains("second"), "and the queue follows it there: {after:?}");

    assert_eq!(harness.respawns().len(), 1, "one respawn: {:?}", harness.spawns());
    assert!(
        harness.respawns()[0].contains(&format!("--resume {SESSION}")),
        "the same conversation, resumed"
    );
    assert_eq!(harness.tokens(), vec![fake_token("b@team")], "under the new account's token");
    assert_eq!(
        harness.registry.host.accounts.session_account_id(SESSION).await.as_deref(),
        Some(b.id.as_str()),
        "the binding moved with it"
    );
    assert_eq!(harness.recorded_fingerprint().and_then(|live| live.account), Some(b.id.clone()));

    let rotation = harness.latest_rotation().expect("the decision is on record");
    assert_eq!(rotation.outcome, RotationOutcome::Switched);
    assert_eq!((rotation.from.id.as_str(), rotation.to.id.as_str()), (a.id.as_str(), b.id.as_str()));
    assert_eq!(rotation.from.label, "a@work");
    assert_eq!(rotation.class, LimitClass::Session);
    assert_eq!(rotation.scope, "all");
    assert_eq!(
        rotation.until_at.0,
        Some(rest::iso_ms(resets * 1000)),
        "the reset the refused event named"
    );
    assert_eq!(rotation.interrupted_tasks, 0);
    assert!(
        staged(&harness.recording(), crate::record::Stage::Rotate),
        "and the flight recording says so"
    );
}

/// Nowhere to go: the only other account is outside the pool. The queue is
/// held — not fed to the refusing account, and not dropped — until somebody
/// sends another message, and then it goes in order.
#[tokio::test]
async fn a_limit_with_nowhere_to_go_holds_the_queue_instead_of_feeding_the_exhausted_account() {
    let home = tempfile::tempdir().expect("tempdir");
    let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("relay", false)], None).await;
    let resets = rest::now_ms() / 1000 + 7200;
    let harness = Harness::rotating(home, &refused_once("five_hour", resets, SESSION_LIMIT, ""), live.clone()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send_with("first", harness.recipe(live.clone()));
    until("the first turn", || harness.state().phase == Phase::Working).await;
    harness.send_with("second", harness.recipe(live.clone()));
    until("the second message to queue", || harness.state().queued == 1).await;
    harness.release();

    until("the decision", || harness.latest_rotation().is_some()).await;
    let rotation = harness.latest_rotation().expect("a decision");
    assert_eq!(rotation.outcome, RotationOutcome::NoCandidate, "an env account is not in the pool");
    assert_eq!(
        rotation.to.id, accounts[0].id,
        "a decision that moved nothing names the account it stayed on"
    );
    assert_eq!(
        harness.injection().map(|run| run.state),
        Some(InjectionRunState::Failed),
        "the refused turn stays failed"
    );

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(harness.seen().len(), 1, "nothing more went to the refusing account: {:?}", harness.seen());
    assert_eq!(harness.state().queued, 1, "and nothing was dropped");
    assert_eq!(harness.state().phase, Phase::Idle);
    assert!(harness.spawns().is_empty(), "and nothing was respawned");
    let journalled = harness.record().payload.hold.expect("the hold is on the record, for the next server");
    assert_eq!(journalled.account, accounts[0].id);
    assert!(journalled.continue_failed);

    // Somebody acts: the held message goes first, then the new one.
    harness.send_with("third", harness.recipe(live.clone()));
    until("the held message to go in", || harness.seen().len() == 2).await;
    assert_eq!(harness.record().payload.hold, None, "and the record stops saying so");
    assert!(harness.seen()[1].contains("second"), "{:?}", harness.seen());
    harness.release();
    until("the new one after it", || harness.seen().len() == 3).await;
    assert!(harness.seen()[2].contains("third"), "{:?}", harness.seen());
    harness.release();
    until("the queue to drain", || {
        harness.state().queued == 0 && harness.state().phase == Phase::Idle
    })
    .await;
}

/// Three switches for one message, and then no more: every account the
/// chain reaches refuses too, and the fourth refusal leaves the run failed
/// rather than bouncing on.
#[tokio::test]
async fn the_attempt_cap_stops_a_message_after_three_switches() {
    let home = tempfile::tempdir().expect("tempdir");
    let (accounts, live) = accounts_in(home.path(), &[("a", true), ("b", true), ("c", true), ("d", true)], None).await;
    let resets = rest::now_ms() / 1000 + 7200;
    let harness = Harness::rotating(home, &refused_once("five_hour", resets, SESSION_LIMIT, ""), live.clone()).await;
    harness.plan_claude(&rotated_claude(SESSION_LIMIT));
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send_with("first", harness.recipe(live.clone()));
    harness.release();
    until("the cap", || {
        harness
            .latest_rotation()
            .is_some_and(|rotation| rotation.outcome == RotationOutcome::CapReached)
    })
    .await;

    let history = harness.registry.host.accounts.rotation_log().history(SESSION, 10);
    let steps: Vec<(RotationOutcome, &str, &str)> = history
        .iter()
        .rev()
        .map(|rotation| (rotation.outcome, rotation.from.label.as_str(), rotation.to.label.as_str()))
        .collect();
    assert_eq!(
        steps,
        vec![
            (RotationOutcome::Switched, "a", "b"),
            (RotationOutcome::Switched, "b", "c"),
            (RotationOutcome::Switched, "c", "d"),
            (RotationOutcome::CapReached, "d", "d"),
        ],
        "each refusal moves on to an account that is not resting, three times"
    );
    assert_eq!(harness.respawns().len(), 3, "{:?}", harness.spawns());
    assert_eq!(harness.tokens(), vec![fake_token("b"), fake_token("c"), fake_token("d")]);
    assert_eq!(harness.after().len(), 3, "one continuation each: {:?}", harness.after());
    assert_eq!(harness.injection().map(|run| run.state), Some(InjectionRunState::Failed));
    assert_eq!(
        harness.registry.host.accounts.session_account_id(SESSION).await.as_deref(),
        Some(accounts[3].id.as_str())
    );
}

/// A limit on the whole account does not wait for background work — it is
/// billing the same exhausted credential — and the continuation names what
/// the restart cut short, with the task's output file.
#[tokio::test]
async fn a_whole_account_limit_switches_at_once_and_lists_the_background_work_it_cut() {
    let home = tempfile::tempdir().expect("tempdir");
    let (_, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], None).await;
    let tasks = home.path().join("task-root").join("a-project").join(SESSION).join("tasks");
    std::fs::create_dir_all(&tasks).expect("tasks dir");
    std::fs::write(tasks.join("bt1.output"), "running\n").expect("output");
    let resets = rest::now_ms() / 1000 + 7200;
    let body = refused_once("five_hour", resets, SESSION_LIMIT, &one_background_task("bt1", "跑完整测试"));
    let harness = Harness::rotating(home, &body, live.clone()).await;
    harness.plan_claude(&rotated_claude(TURN_OK));
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send_with("first", harness.recipe(live.clone()));
    harness.release();
    until("the continuation", || harness.after().len() == 1).await;

    let continuation = &harness.after()[0];
    assert!(
        continuation.contains("以下后台子任务被中断，请检查它们的输出后按需重新派发："),
        "{continuation}"
    );
    assert!(continuation.contains("- 跑完整测试（输出："), "{continuation}");
    assert!(continuation.contains("bt1.output"), "{continuation}");
    let rotation = harness.latest_rotation().expect("a decision");
    assert_eq!(rotation.interrupted_tasks, 1);
    assert_eq!(harness.respawns().len(), 1);
    until("the replacement to settle", || harness.state().phase == Phase::Idle).await;
    assert_eq!(harness.state().background, 0, "the tasks went with the old process");
}

/// A limit on one model family lets claude's background work finish first
/// — a subagent on another model can still complete — and only then moves
/// the session.
#[tokio::test]
async fn a_family_limit_waits_for_background_work_before_switching() {
    let home = tempfile::tempdir().expect("tempdir");
    let (_, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], Some("fable")).await;
    let resets = rest::now_ms() / 1000 + 3 * 86_400;
    let finishes = r#"
(
  await_file tasks-done
  printf '%s\n' '{"type":"system","subtype":"background_tasks_changed","tasks":[]}'
) &
"#;
    let body = format!(
        "{finishes}{}",
        refused_once(
            "seven_day_overage_included",
            resets,
            FABLE_LIMIT,
            &one_background_task("bt1", "Review the diff")
        )
    );
    let harness = Harness::rotating(home, &body, live.clone()).await;
    harness.plan_claude(&rotated_claude(TURN_OK));
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send_with("first", harness.recipe(live.clone()));
    harness.release();
    until("the decision", || harness.latest_rotation().is_some()).await;
    let rotation = harness.latest_rotation().expect("a decision");
    assert_eq!((rotation.class, rotation.scope.as_str()), (LimitClass::Weekly, "fable"));
    assert_eq!(rotation.until_at.0, Some(rest::iso_ms(resets * 1000)), "the reset the event named");
    until("the switch to park behind the background work", || harness.state().awaiting_background).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(harness.spawns().is_empty(), "no restart while the task runs: {:?}", harness.spawns());
    assert!(harness.after().is_empty());

    harness.nudge("tasks-done");
    until("the continuation, once the task is done", || harness.after().len() == 1).await;
    let continuation = &harness.after()[0];
    assert!(continuation.contains("a@work 的 Fable 每周额度用完"), "{continuation}");
    assert!(!continuation.contains("以下后台子任务"), "nothing was cut short: {continuation}");
    assert_eq!(harness.latest_rotation().expect("a decision").interrupted_tasks, 0);
    assert_eq!(harness.tokens(), vec![fake_token("b@team")]);
}

/// A rest stands in the way of its own family only, and the family is the
/// one of the model the process runs: a fable session limited on its
/// account moves to the only other pool account, which rests under opus.
#[tokio::test]
async fn a_limited_session_moves_past_a_rest_on_another_family() {
    let home = tempfile::tempdir().expect("tempdir");
    let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], Some("fable")).await;
    let b = accounts[1].clone();
    let resets = rest::now_ms() / 1000 + 7200;
    let harness = Harness::rotating(home, &refused_once("five_hour", resets, SESSION_LIMIT, ""), live.clone()).await;
    rest_on(&harness.registry.host, &b.id, LimitClass::Weekly, "opus", rest::now_ms() + 3 * 86_400_000);
    harness.plan_claude(&rotated_claude(TURN_OK));
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send_with("first", harness.recipe(live.clone()));
    harness.release();
    until("the decision", || harness.latest_rotation().is_some()).await;
    let rotation = harness.latest_rotation().expect("a decision");
    assert_eq!(
        (rotation.outcome, rotation.to.id.as_str()),
        (RotationOutcome::Switched, b.id.as_str()),
        "b's opus rest is no reason to hold a fable session"
    );
    until("the continuation", || harness.after().len() == 1).await;
    assert_eq!(harness.tokens(), vec![fake_token("b@team")]);
}

/// Rest `account` under `scope` until `until_ms`, as phase 1 would have.
fn rest_on(registry: &RunHost, account: &str, class: LimitClass, scope: &str, until_ms: i64) {
    registry
        .accounts
        .limit_log()
        .record(
            &Observation {
                account_id: account.to_string(),
                session_id: Some("elsewhere".into()),
                source: "claude:result",
                model: None,
                verdict: Verdict {
                    class,
                    scope: scope.to_string(),
                    until_ms: Some(until_ms),
                },
                raw: "You have hit your session limit".into(),
                reason: "You have hit your session limit".into(),
                may_rest: true,
            },
            rest::now_ms(),
        )
        .expect("rest");
}

/// A message as the journal keeps it.
fn journalled(id: &str, text: &str, fingerprint: &Fingerprint, rotation_attempt: u8, continuation: Option<Continuation>) -> Queued {
    Queued {
        id: id.to_string(),
        text: text.to_string(),
        images: Vec::new(),
        files: Vec::new(),
        fingerprint: fingerprint.clone(),
        steered: false,
        rotation_attempt,
        continuation,
    }
}

/// M2: a server restarted while its queue was held keeps holding it. The
/// pump used to feed the queue to the account that had refused the moment
/// the new server attached, fail it, and decide — and notify — all over
/// again. The watchdog's re-checks while the account still rests decide
/// nothing either.
#[tokio::test]
async fn a_restart_during_a_hold_keeps_the_queue_held() {
    let home = tempfile::tempdir().expect("tempdir");
    let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("relay", false)], None).await;
    let a = accounts[0].clone();
    let lifts = rest::now_ms() + 2 * 3600 * 1000;
    let hold = Hold {
        account: a.id.clone(),
        label: a.label.clone(),
        class: LimitClass::Session,
        scope: "all".into(),
        until_ms: Some(lifts),
        switches: 0,
        continue_failed: true,
    };
    let queued = vec![journalled("q-1", "second", &live, 0, None)];
    let harness = Harness::rotating_with(
        home,
        hold_until_released(),
        live.clone(),
        Duration::from_millis(50),
        queued,
        Some(hold.clone()),
    )
    .await;
    rest_on(&harness.registry.host, &a.id, LimitClass::Session, "all", lifts);
    until("the init", || harness.state().phase == Phase::Idle).await;

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        harness.seen().is_empty(),
        "nothing went to the account that refused: {:?}",
        harness.seen()
    );
    assert_eq!(harness.state().queued, 1, "and nothing was dropped");
    assert!(
        harness.rotations().is_empty(),
        "no second decision about the same limit, however often it was re-checked"
    );
    assert_eq!(harness.record().payload.hold, Some(hold), "and the record still says it is held");
}

/// M1: a server restarted while a continuation waited behind background
/// work. The continuation and the message retargeted behind it name the new
/// account, the running process is still on the old one, and the journal
/// cannot carry an environment — so they used to be dropped at adoption.
/// Now they are kept, rebuilt for the account the session is bound to, and
/// delivered there once the background work is done.
#[tokio::test]
async fn a_restart_during_a_family_wait_keeps_the_continuation_and_moves_to_the_new_account() {
    let home = tempfile::tempdir().expect("tempdir");
    let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], Some("fable")).await;
    let b = accounts[1].clone();
    let store = droppen_accounts::store::AccountsStore::new(Arc::new(Paths::new(home.path().to_path_buf())), logins());
    store
        .bind_session_account(SESSION, Some(&b.id), AccountProvider::Claude)
        .await
        .expect("bind");
    let under_b = Fingerprint {
        account: Some(b.id.clone()),
        credential: store.credential_hash(&b.id).await,
        ..live.clone()
    };
    let why = rotation::reason(LimitClass::Weekly, "fable");
    let note = Continuation {
        record: None,
        from: "a@work".into(),
        to: "b@team".into(),
        reason: why.clone(),
        until: None,
    };
    let queued = vec![
        journalled(
            "c-1",
            &rotation::continuation_prompt("a@work", "b@team", &why, None, &[]),
            &under_b,
            1,
            Some(note),
        ),
        journalled("q-2", "second", &under_b, 0, None),
    ];
    let finishes = r#"
(
  await_file tasks-done
  printf '%s\n' '{"type":"system","subtype":"background_tasks_changed","tasks":[]}'
) &
"#;
    let harness = Harness::build_in(
        home,
        &history_with_an_orphan_task(),
        &format!("{finishes}{}", hold_until_released()),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
        Duration::from_millis(120),
        NEVER_SETTLES,
        Adoption {
            attach: Some(Witness::Fresh),
            live: Some(live.clone()),
            queued,
            recipe: Some(live.clone()),
            hold: None,
            ring: None,
            transcript: None,
            ..Adoption::default()
        },
    )
    .await;
    harness.plan_claude(&rotated_claude(TURN_OK));
    until("the continuation to park behind the background work", || {
        harness.state().awaiting_background
    })
    .await;
    assert_eq!(harness.state().queued, 2, "both survived the restart: {:?}", harness.queued());
    assert!(harness.queued()[0].text.contains("自动切换到 b@team"));
    assert!(harness.spawns().is_empty(), "and nothing was restarted while the task ran");

    // The task ends: its transcript goes quiet and claude says so.
    let subagents = crate::sessions::transcript::subagent_dir(&harness.transcript());
    quiet(&subagents.join("agent-ghost.jsonl"));
    quiet(&subagents);
    harness.nudge("tasks-done");
    until("the continuation and the message behind it to reach the new account", || {
        harness.after().len() == 2
    })
    .await;
    let after = harness.after();
    assert!(after[0].contains("a@work 的 Fable 每周额度用完"), "{}", after[0]);
    assert!(after[1].contains("second"), "{after:?}");
    assert!(harness.seen().is_empty(), "the old account got neither: {:?}", harness.seen());
    assert_eq!(harness.tokens(), vec![fake_token("b@team")], "under the account the session is bound to");
    assert!(
        harness.rotations().is_empty(),
        "the decision was the last server's; this one only carried it out"
    );
}

/// A claude that is refused once, holds, and then — on a nudge — runs a
/// turn of its own (a background task finishing, say) that the same
/// account refuses again.
fn refused_twice(resets_at: i64) -> String {
    format!(
        r#"
dir=$(dirname "$GO")
(
  await_file again
  if [ -f "$dir/again" ]; then
printf '%s\n' '{{"type":"assistant","message":{{}}}}'
printf '%s\n' '{{"type":"rate_limit_event","rate_limit_info":{{"status":"rejected","rateLimitType":"five_hour","resetsAt":{resets_at}}}}}'
printf '%s\n' '{SESSION_LIMIT}'
  fi
) &
{}
"#,
        refused_once("five_hour", resets_at, SESSION_LIMIT, "")
    )
}

/// L1: while the queue is held against an account, another refusal from it
/// is the same limit. No second decision, no second notification.
#[tokio::test]
async fn a_second_refusal_during_a_hold_decides_nothing_new() {
    let home = tempfile::tempdir().expect("tempdir");
    let (_, live) = accounts_in(home.path(), &[("a@work", true), ("relay", false)], None).await;
    let resets = rest::now_ms() / 1000 + 7200;
    let harness = Harness::rotating(home, &refused_twice(resets), live.clone()).await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send_with("first", harness.recipe(live.clone()));
    until("the first turn", || harness.state().phase == Phase::Working).await;
    harness.send_with("second", harness.recipe(live.clone()));
    harness.release();
    until("the hold", || harness.rotations().len() == 1).await;

    harness.nudge("again");
    until("claude's own turn to be refused too", || {
        harness.state().phase == Phase::Idle && harness.injection().is_some_and(|run| run.state == InjectionRunState::Failed)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(harness.rotations().len(), 1, "one decision about one limit: {:?}", harness.rotations());
    assert_eq!(harness.state().queued, 1, "and the queue is still held");
    assert_eq!(harness.seen().len(), 1, "{:?}", harness.seen());
}

/// L1: a held queue asks again every tick, and moves the moment an account
/// frees up — here, the only other pool account's rest being cleared.
#[tokio::test]
async fn a_held_queue_moves_as_soon_as_an_account_frees_up() {
    let home = tempfile::tempdir().expect("tempdir");
    let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], None).await;
    let b = accounts[1].clone();
    let resets = rest::now_ms() / 1000 + 7200;
    let harness = Harness::rotating_with(
        home,
        &refused_once("five_hour", resets, SESSION_LIMIT, ""),
        live.clone(),
        Duration::from_millis(50),
        Vec::new(),
        None,
    )
    .await;
    rest_on(&harness.registry.host, &b.id, LimitClass::Session, "all", resets * 1000);
    harness.plan_claude(&rotated_claude(TURN_OK));
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send_with("first", harness.recipe(live.clone()));
    until("the first turn", || harness.state().phase == Phase::Working).await;
    harness.send_with("second", harness.recipe(live.clone()));
    harness.release();
    until("the hold", || harness.rotations().len() == 1).await;
    assert_eq!(harness.rotations()[0].outcome, RotationOutcome::NoCandidate, "b was resting");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(harness.spawns().is_empty(), "and a re-check with nothing new to say moves nothing");

    harness.registry.host.accounts.limit_log().clear(&b.id, None).expect("clear");
    until("the continuation and the held message to reach b", || harness.after().len() == 2).await;
    let after = harness.after();
    assert!(after[0].contains("自动切换到 b@team"), "{}", after[0]);
    assert!(after[1].contains("second"), "{after:?}");
    let outcomes: Vec<RotationOutcome> = harness.rotations().iter().map(|rotation| rotation.outcome).collect();
    assert_eq!(outcomes, vec![RotationOutcome::Switched, RotationOutcome::NoCandidate], "newest first");
    assert_eq!(harness.tokens(), vec![fake_token("b@team")]);
}

/// L4: nothing freed up, and the held account's own limit lifted. The
/// failed message is continued there before the queue goes.
#[tokio::test]
async fn a_lifted_hold_continues_the_failed_message_on_the_same_account() {
    let home = tempfile::tempdir().expect("tempdir");
    let (_, live) = accounts_in(home.path(), &[("a@work", true), ("relay", false)], None).await;
    // A five-hour window that resets in a couple of seconds, said by the
    // result too — the older CLI's epoch marker — so the rest is that short
    // rather than the class default a result with no time would give it.
    let resets = rest::now_ms() / 1000 + 2;
    let result = format!(
        r#"{{"type":"result","subtype":"success","is_error":true,"result":"Claude AI usage limit reached|{resets}","api_error_status":429}}"#
    );
    let harness = Harness::rotating_with(
        home,
        &refused_once("five_hour", resets, &result, ""),
        live.clone(),
        Duration::from_millis(50),
        Vec::new(),
        None,
    )
    .await;
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send_with("first", harness.recipe(live.clone()));
    until("the first turn", || harness.state().phase == Phase::Working).await;
    harness.send_with("second", harness.recipe(live.clone()));
    harness.release();
    until("the hold", || harness.rotations().len() == 1).await;

    until("the continuation, once the window resets", || harness.seen().len() == 2).await;
    let seen = harness.seen();
    assert!(
        seen[1].contains("[Droppen] 账号 a@work 的额度已恢复。上一条消息因额度用完没有完成，请从中断处继续。"),
        "{}",
        seen[1]
    );
    harness.release();
    until("then the held message", || harness.seen().len() == 3).await;
    assert!(harness.seen()[2].contains("second"), "{:?}", harness.seen());
    harness.release();
    until("the queue to drain", || {
        harness.state().queued == 0 && harness.state().phase == Phase::Idle
    })
    .await;
    assert!(harness.spawns().is_empty(), "the same process, the same account");
    assert_eq!(harness.rotations().len(), 1, "and no decision about the lift");
}

/// L5: somebody moved the session to another account by hand while the
/// turn was running. The limit sends it there — not to whatever the pool
/// would have chosen.
#[tokio::test]
async fn a_binding_changed_by_hand_is_where_a_limited_session_goes() {
    let home = tempfile::tempdir().expect("tempdir");
    let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true), ("c@home", true)], None).await;
    let c = accounts[2].clone();
    let resets = rest::now_ms() / 1000 + 7200;
    let harness = Harness::rotating(home, &refused_once("five_hour", resets, SESSION_LIMIT, ""), live.clone()).await;
    harness.plan_claude(&rotated_claude(TURN_OK));
    until("the init", || harness.state().phase == Phase::Idle).await;
    harness.send_with("first", harness.recipe(live.clone()));
    until("the first turn", || harness.state().phase == Phase::Working).await;
    harness
        .registry
        .host.accounts
        .bind_session_account(SESSION, Some(&c.id), AccountProvider::Claude)
        .await
        .expect("bind by hand");
    harness.release();

    until("the continuation", || harness.after().len() == 1).await;
    assert!(harness.after()[0].contains("自动切换到 c@home"), "{}", harness.after()[0]);
    assert_eq!(
        harness.tokens(),
        vec![fake_token("c@home")],
        "the account that was chosen, not the pool's first"
    );
    let rotation = harness.latest_rotation().expect("a decision");
    assert_eq!((rotation.outcome, rotation.to.id.as_str()), (RotationOutcome::Switched, c.id.as_str()));
}

// ------------------------------------------------- ahead of a limit

/// A claude whose every turn ends normally after an `allowed_warning`
/// reading — the status a window nearing its cap carries — with the
/// five-hour window at `utilization` and the week at 40 %. `before` is
/// written ahead of the first turn's reading, for a background task.
fn warns_every_turn(utilization: f64, resets_at: i64, before: &str) -> String {
    let week = resets_at + 4 * 86_400;
    format!(
        r#"
turns=0
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in *control_request*) continue ;; esac
  turns=$((turns + 1))
  await_go
  rm -f "$GO"
  if [ "$turns" = 1 ]; then
{before}
:
  fi
  printf '%s\n' '{{"type":"rate_limit_event","rate_limit_info":{{"status":"allowed_warning","rateLimitType":"five_hour","resetsAt":{resets_at},"unifiedWindows":{{"five_hour":{{"resetsAt":{resets_at},"utilization":{utilization}}},"seven_day":{{"resetsAt":{week},"utilization":0.4}}}}}}}}'
  printf '%s\n' '{TURN_OK}'
done
"#
    )
}

/// File a fresh reading of `account` with the five-hour window at `five`
/// and the week at `week`, both fractions, as a session on it would.
fn read_account(registry: &RunHost, account: &str, five: f64, week: f64) {
    let resets = rest::now_ms() / 1000 + 3600;
    let info = serde_json::json!({
        "unifiedWindows": {
            "five_hour": {"resetsAt": resets, "utilization": five},
            "seven_day": {"resetsAt": resets + 86_400, "utilization": week},
        }
    });
    let windows = info.windows_from_rate_limit_info();
    registry
        .quota
        .ingest_session(droppen_accounts::quota::UsageLogin::Account(account.to_string()), windows);
}

/// The whole of a move ahead of a limit, on the everyday case: a turn ends
/// normally with the five-hour window at 96 % and a message queued behind
/// it. The session moves to the account with room before that message
/// goes in, and the message — not a continuation, because nothing failed
/// — is the first thing the new account's process reads.
#[tokio::test]
async fn a_normal_turn_end_with_a_near_window_moves_the_session_ahead_and_the_next_message_runs_there() {
    let home = tempfile::tempdir().expect("tempdir");
    let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], None).await;
    let (a, b) = (accounts[0].clone(), accounts[1].clone());
    let resets = rest::now_ms() / 1000 + 5400;
    let harness = Harness::rotating(home, &warns_every_turn(0.96, resets, ""), live.clone()).await;
    read_account(&harness.registry.host, &b.id, 0.10, 0.20);
    harness.plan_claude(&rotated_claude(TURN_OK));
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send_with("first", harness.recipe(live.clone()));
    until("the first turn", || harness.state().phase == Phase::Working).await;
    harness.send_with("second", harness.recipe(live.clone()));
    until("the second message to queue", || harness.state().queued == 1).await;
    harness.release();

    until("the queued message to reach the new account", || {
        harness.after().len() == 1 && harness.state().phase == Phase::Idle
    })
    .await;
    assert_eq!(
        harness.injection().map(|run| run.state),
        Some(InjectionRunState::Ok),
        "nothing failed: the move was made between turns"
    );
    let seen = harness.seen();
    assert_eq!(seen.len(), 1, "the account running out got the first message and nothing else: {seen:?}");
    let after = harness.after();
    assert!(after[0].contains("second"), "the queued message is what goes in: {after:?}");
    assert!(!after[0].contains("[Droppen]"), "and no continuation prompt goes before it: {after:?}");

    assert_eq!(harness.respawns().len(), 1, "one respawn: {:?}", harness.spawns());
    assert!(
        harness.respawns()[0].contains(&format!("--resume {SESSION}")),
        "the same conversation, resumed"
    );
    assert_eq!(harness.tokens(), vec![fake_token("b@team")], "under the new account's token");
    assert_eq!(
        harness.registry.host.accounts.session_account_id(SESSION).await.as_deref(),
        Some(b.id.as_str()),
        "the binding moved with it"
    );

    let rotation = harness.latest_rotation().expect("the decision is on record");
    assert_eq!(rotation.trigger, RotationTrigger::Ahead);
    assert_eq!(rotation.outcome, RotationOutcome::Switched);
    assert_eq!((rotation.from.id.as_str(), rotation.to.id.as_str()), (a.id.as_str(), b.id.as_str()));
    assert_eq!(
        (rotation.class, rotation.scope.as_str()),
        (LimitClass::Session, "all"),
        "the five-hour window"
    );
    assert_eq!(rotation.used_percent, Some(96.0));
    assert_eq!(rotation.until_at.0, Some(rest::iso_ms(resets * 1000)), "when that window resets");
    assert_eq!(rotation.interrupted_tasks, 0);
    assert!(
        staged(&harness.recording(), crate::record::Stage::Rotate),
        "and the flight recording says so"
    );

    // On the new account nothing is near, and nothing moves again. The
    // next message is built for the binding, as the send route builds it.
    let under_b = Fingerprint {
        account: Some(b.id.clone()),
        credential: harness.registry.host.accounts.credential_hash(&b.id).await,
        ..live.clone()
    };
    harness.send_with("third", harness.recipe(under_b));
    until("the third message, on the same new process", || harness.after().len() == 2).await;
    assert_eq!(harness.respawns().len(), 1, "{:?}", harness.spawns());
    assert_eq!(harness.rotations().len(), 1, "{:?}", harness.rotations());
}

/// Ahead of a limit, too, a rest stands in the way of its own family
/// only: a fable session whose five-hour window is near moves to the only
/// other pool account, which has room and rests under opus.
#[tokio::test]
async fn a_session_moves_ahead_past_a_rest_on_another_family() {
    let home = tempfile::tempdir().expect("tempdir");
    let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], Some("fable")).await;
    let b = accounts[1].clone();
    let resets = rest::now_ms() / 1000 + 5400;
    let harness = Harness::rotating(home, &warns_every_turn(0.96, resets, ""), live.clone()).await;
    read_account(&harness.registry.host, &b.id, 0.10, 0.20);
    rest_on(&harness.registry.host, &b.id, LimitClass::Weekly, "opus", rest::now_ms() + 3 * 86_400_000);
    harness.plan_claude(&rotated_claude(TURN_OK));
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send_with("first", harness.recipe(live.clone()));
    until("the first turn", || harness.state().phase == Phase::Working).await;
    harness.send_with("second", harness.recipe(live.clone()));
    until("the second message to queue", || harness.state().queued == 1).await;
    harness.release();

    until("the second message to go in", || harness.seen().len() + harness.after().len() == 2).await;
    assert_eq!(
        harness.tokens(),
        vec![fake_token("b@team")],
        "b's opus rest is no reason to keep a fable session on the account running out"
    );
    let rotation = harness.latest_rotation().expect("the decision is on record");
    assert_eq!((rotation.trigger, rotation.to.id.as_str()), (RotationTrigger::Ahead, b.id.as_str()));
}

/// The point of moving early is that nothing is cut off, so a turn that
/// leaves a background task running does not move the session — not at its
/// end, and not before the next turn while the task still runs. Once the
/// task is done, the next quiet point moves it.
#[tokio::test]
async fn a_move_ahead_waits_for_background_work_and_happens_at_the_next_quiet_point() {
    let home = tempfile::tempdir().expect("tempdir");
    let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], None).await;
    let b = accounts[1].clone();
    let resets = rest::now_ms() / 1000 + 5400;
    let finishes = r#"
(
  await_file tasks-done
  printf '%s\n' '{"type":"system","subtype":"background_tasks_changed","tasks":[]}'
) &
"#;
    let body = format!(
        "{finishes}{}",
        warns_every_turn(0.97, resets, &one_background_task("bt1", "Review the diff"))
    );
    let harness = Harness::rotating(home, &body, live.clone()).await;
    read_account(&harness.registry.host, &b.id, 0.05, 0.05);
    harness.plan_claude(&rotated_claude(TURN_OK));
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send_with("first", harness.recipe(live.clone()));
    until("the first turn", || harness.state().phase == Phase::Working).await;
    harness.send_with("second", harness.recipe(live.clone()));
    until("the second message to queue", || harness.state().queued == 1).await;
    harness.release();

    until("the second message to go to the same process", || harness.seen().len() == 2).await;
    assert_eq!(harness.state().background, 1, "the task is still running");
    harness.release();
    until("the second turn to end", || {
        harness.state().phase == Phase::Idle && harness.state().queued == 0
    })
    .await;
    assert!(
        harness.rotations().is_empty(),
        "no move while background work runs: {:?}",
        harness.rotations()
    );
    assert!(harness.spawns().is_empty(), "and no restart: {:?}", harness.spawns());

    harness.nudge("tasks-done");
    until("the task to finish", || harness.state().background == 0).await;
    harness.send_with("third", harness.recipe(live.clone()));
    until("the third message to reach the new account", || harness.after().len() == 1).await;
    assert!(harness.after()[0].contains("third"), "{:?}", harness.after());
    assert_eq!(harness.seen().len(), 2, "the old account never saw it: {:?}", harness.seen());
    assert_eq!(harness.tokens(), vec![fake_token("b@team")]);
    let rotation = harness.latest_rotation().expect("a decision");
    assert_eq!((rotation.trigger, rotation.outcome), (RotationTrigger::Ahead, RotationOutcome::Switched));
    assert_eq!(rotation.used_percent, Some(97.0));
}

/// Nowhere known to be better: the session stays, nothing is written down
/// and nothing is pushed — a near window is not news on its own.
#[tokio::test]
async fn a_near_window_with_no_known_good_account_leaves_the_session_where_it_is() {
    let home = tempfile::tempdir().expect("tempdir");
    let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], None).await;
    let b = accounts[1].clone();
    let resets = rest::now_ms() / 1000 + 5400;
    let harness = Harness::rotating(home, &warns_every_turn(0.99, resets, ""), live.clone()).await;
    // b is close to its own line: 88 % is not under 95 − 10.
    read_account(&harness.registry.host, &b.id, 0.88, 0.10);
    until("the init", || harness.state().phase == Phase::Idle).await;

    harness.send_with("first", harness.recipe(live.clone()));
    until("the first turn", || harness.state().phase == Phase::Working).await;
    harness.send_with("second", harness.recipe(live.clone()));
    harness.release();
    until("the second message on the same process", || harness.seen().len() == 2).await;
    harness.release();
    until("the queue to drain", || {
        harness.state().phase == Phase::Idle && harness.state().queued == 0
    })
    .await;
    assert!(harness.rotations().is_empty(), "{:?}", harness.rotations());
    assert!(harness.spawns().is_empty());
    assert_eq!(harness.injection().map(|run| run.state), Some(InjectionRunState::Ok));
}
