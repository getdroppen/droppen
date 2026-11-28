//! End-to-end tests driving `tests/support/fake_codex.py` through `CodexRuns`.

use super::*;

/// A registry whose `codex` is `tests/support/fake_codex.py`, which logs
/// every line it reads to `codex-stdin.jsonl` in its working directory.
/// `home` becomes the test root the fake insists on; the guard puts the
/// child `PATH` back when it goes.
fn fake_codex_registry(home: &std::path::Path, instance: &str) -> (droppen_base::process_env::ChildPathGuard, Arc<InjectionRegistry>) {
    let guard = droppen_base::process_env::ChildPathGuard::take();
    // The stand-in app server runs only under a test root.
    std::fs::write(home.join(".rh-test-root"), b"").expect("test root");
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    let codex = bin.join("codex");
    let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/fake_codex.py");
    std::fs::write(&codex, format!("#!/bin/sh\nexec python3 {fake} \"$@\"\n")).expect("codex");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let path = format!("{}:{}", bin.display(), droppen_base::process_env::child_path());
    droppen_base::process_env::set_child_path(Some(path));
    let paths = Arc::new(Paths::new(home.into()));
    let dirs = InstanceDirs::new(&paths.data_dir(), instance).unwrap();
    dirs.create().unwrap();
    let accounts = Arc::new(droppen_accounts::store::AccountsStore::new(paths.clone(), logins()));
    let quota = Arc::new(droppen_accounts::quota::QuotaCache::new(paths.clone(), logins()));
    let decoration = droppen_provider::testing::no_decoration();
    (guard, InjectionRegistry::new(paths, &dirs, accounts, quota, decoration))
}

/// The Codex runs `providers` hold.
pub(super) fn codex(providers: &droppen_provider::ProviderRegistry) -> &Arc<CodexRuns> {
    providers.provider::<Codex>().expect("Codex").runs()
}

/// A step of a spawn, as [`droppen_provider::testing::Step`] records it.
fn step(what: &'static str, gate: bool, claim: Option<bool>) -> droppen_provider::testing::Step {
    droppen_provider::testing::Step { what, gate, claim }
}

/// The spawn gate only reserves a Codex session; its app server starts
/// outside it. A new thread takes the gate only to take its permit, and is
/// claimed once the app server has named it. A thread this server has no process for is
/// reserved under the gate, then claimed and started outside it. A process
/// retired for another account is retired outside the gate too, and lets
/// its claim go before it answers. An unloaded session is claimed under the
/// gate and archived with `codex archive` outside it, and the claim goes first.
/// A resume the app server answers with another thread lets its claim go
/// before the app server's stdin closes.
#[tokio::test]
async fn codex_reserves_under_the_gate_and_starts_and_retires_outside_it() {
    const RESUMED: &str = "0199c1a1-0000-4000-8000-000000000001";
    const UNLOADED: &str = "0199c1a1-0000-4000-8000-000000000002";
    const MISMATCHED: &str = "0199c1a1-0000-4000-8000-000000000003";
    const OTHER: &str = "0199c1a1-0000-4000-8000-000000000004";
    let home = tempfile::tempdir().expect("tempdir");
    let (_path, registry) = fake_codex_registry(home.path(), "codex-steps");
    let providers = codex_only(&registry);
    let gate = providers.gate();
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).expect("work");
    let work = work.to_string_lossy().into_owned();
    let plain = SendOptions::default();

    let (id, _) = codex(&providers).codex_deliver(gate, None, Some(&work), "hello", &plain, None).await.expect("a new thread");
    assert_eq!(
        registry.host.steps(),
        [
            step("codex started", false, None),
            step("codex thread", false, Some(true)),
            step("codex mapped", false, Some(true)),
        ]
    );
    assert!(!registry.host.gate_held());
    assert!(registry.host.claim_held(Source::Codex, &id), "the driver holds the claim");

    for _ in 0..500 {
        if codex(&providers).codex_handle(&id).is_some_and(|handle| !handle.busy()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let other = CodexAccount {
        id: "another".to_string(),
        launch: droppen_accounts::store::AccountLaunch {
            provider: AccountProvider::Codex,
            env: vec![],
            codex_args: vec![],
        },
    };
    codex(&providers).codex_deliver(gate, Some(&id), Some(&work), "again", &plain, Some(other))
        .await
        .expect("resumed under the other account");
    assert_eq!(
        registry.host.steps()[3..],
        [
            // The old process still holds the session until it retires.
            step("codex reserved", true, Some(true)),
            step("codex retired", false, Some(false)),
            step("codex started", false, Some(true)),
            step("codex thread", false, Some(true)),
            step("codex mapped", false, Some(true)),
        ]
    );

    codex(&providers).codex_deliver(gate, Some(RESUMED), Some(&work), "resume", &plain, None).await.expect("resumed");
    assert_eq!(
        registry.host.steps()[8..],
        [
            step("codex reserved", true, Some(false)),
            step("codex started", false, Some(true)),
            step("codex thread", false, Some(true)),
            step("codex mapped", false, Some(true)),
        ]
    );

    codex(&providers).archive_codex(gate, UNLOADED).await.expect("archived");
    assert_eq!(registry.host.steps()[12..], [step("codex archive", false, Some(true))]);
    assert!(!registry.host.gate_held());
    assert!(!registry.host.claim_held(Source::Codex, UNLOADED), "the claim went with the archive");

    let elsewhere = home.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("elsewhere");
    std::fs::write(elsewhere.join("resume-thread.txt"), OTHER).expect("resume answer");
    let elsewhere = elsewhere.to_string_lossy().into_owned();
    let refused = codex(&providers).codex_deliver(gate, Some(MISMATCHED), Some(&elsewhere), "resume", &plain, None)
        .await
        .expect_err("another thread is refused");
    assert_eq!(refused.to_string(), "Codex resumed a different thread");
    assert_eq!(
        registry.host.steps()[13..],
        [
            step("codex reserved", true, Some(false)),
            step("codex started", false, Some(true)),
            step("codex setup failed", false, Some(false)),
        ]
    );
    assert!(!registry.host.gate_held());
    assert!(!registry.host.claim_held(Source::Codex, MISMATCHED));

    for thread in [id.as_str(), RESUMED] {
        if let Some(handle) = codex(&providers).codex_handle(thread) {
            for _ in 0..500 {
                if !handle.busy() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            retire(&handle).await.expect("retired");
        }
    }
}

/// A scratch home for the tests of a start outside the gate: the fake
/// codex's registry and providers, and its working directory.
fn unheld(name: &str) -> (tempfile::TempDir, droppen_base::process_env::ChildPathGuard, Arc<InjectionRegistry>, Arc<droppen_provider::ProviderRegistry>, std::path::PathBuf) {
    let home = tempfile::tempdir().expect("tempdir");
    let (path, registry) = fake_codex_registry(home.path(), name);
    let providers = codex_only(&registry);
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).expect("work");
    (home, path, registry, providers, work)
}

/// Wait until the steps taken so far include `what` `times` times.
async fn wait_for_step(registry: &InjectionRegistry, what: &str, times: usize) {
    for _ in 0..1000 {
        if registry.host.steps().iter().filter(|step| step.what == what).count() >= times {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no {what:?} ({times}) in {:?}", registry.host.steps());
}

/// Every app server's `turn/start` and `turn/steer` texts, as codex read them, in order.
fn sent_texts(work: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(work.join("codex-stdin.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("json"))
        .filter(|request| matches!(request["method"].as_str(), Some("turn/start" | "turn/steer")))
        .filter_map(|request| request["params"]["input"][0]["text"].as_str().map(str::to_owned))
        .collect()
}

/// Retire every app server the test left running.
async fn retire_all(runs: &CodexRuns, ids: &[&str]) {
    for id in ids {
        if let Some(handle) = runs.codex_handle(id).filter(Handle::alive) {
            for _ in 0..500 {
                if !handle.busy() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            retire(&handle).await.expect("retired");
        }
    }
}

/// An app server resuming a thread starts outside the spawn gate: another
/// send takes the gate while it is held at its first answer. The session is
/// reserved meanwhile, as starting, and a second message for it waits for
/// that app server rather than starting another: one start, both messages
/// delivered, in the order they were sent.
#[tokio::test]
async fn a_codex_start_holds_no_gate_and_takes_what_is_sent_to_it_meanwhile() {
    const RESUMED: &str = "0199c1a1-0000-4000-8000-000000000005";
    let (_home, _path, registry, providers, work) = unheld("codex-unheld");
    std::fs::write(work.join("init-hold"), "").expect("hold");
    let runs = codex(&providers).clone();
    let send = |text: &'static str| {
        let (runs, providers, work) = (runs.clone(), providers.clone(), work.to_string_lossy().into_owned());
        tokio::spawn(async move { runs.codex_deliver(providers.gate(), Some(RESUMED), Some(&work), text, &SendOptions::default(), None).await })
    };
    let first = send("first");
    wait_for_step(&registry, "codex started", 1).await;
    assert!(!registry.host.gate_held(), "the start holds no gate");
    drop(tokio::time::timeout(Duration::from_secs(1), providers.gate().permit()).await.expect("another send takes the gate meanwhile"));
    assert_eq!(runs.codex_handle(RESUMED).expect("reserved").state().phase, Phase::Starting);

    let second = send("second");
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    std::fs::remove_file(work.join("init-hold")).expect("release");
    first.await.expect("joined").expect("the first is delivered");
    second.await.expect("joined").expect("the second is delivered");
    assert_eq!(registry.host.steps().iter().filter(|step| step.what == "codex started").count(), 1, "one app server");
    for _ in 0..500 {
        if sent_texts(&work).len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(sent_texts(&work), ["first", "second"]);
    retire_all(&runs, &[RESUMED]).await;
}

/// A new thread holds the gate only to take its permit: while its app
/// server is held at its first answer, another send takes the gate.
#[tokio::test]
async fn a_new_codex_thread_starts_outside_the_gate() {
    let (_home, _path, registry, providers, work) = unheld("codex-new-unheld");
    std::fs::write(work.join("init-hold"), "").expect("hold");
    let start = {
        let (runs, providers, work) = (codex(&providers).clone(), providers.clone(), work.to_string_lossy().into_owned());
        tokio::spawn(async move { runs.codex_deliver(providers.gate(), None, Some(&work), "hello", &SendOptions::default(), None).await })
    };
    wait_for_step(&registry, "codex started", 1).await;
    assert!(!registry.host.gate_held(), "the start holds no gate");
    drop(tokio::time::timeout(Duration::from_secs(1), providers.gate().permit()).await.expect("another send takes the gate meanwhile"));
    std::fs::remove_file(work.join("init-hold")).expect("release");
    let (id, _) = start.await.expect("joined").expect("started");
    assert!(registry.host.claim_held(Source::Codex, &id), "claimed once codex named it");
    retire_all(codex(&providers), &[&id]).await;
}

/// A start that fails gives the session back as it found it: no handle,
/// no claim, and a message sent to it meanwhile is answered, not kept.
#[tokio::test]
async fn a_codex_start_that_fails_gives_the_session_back() {
    const MISMATCHED: &str = "0199c1a1-0000-4000-8000-000000000006";
    let (_home, _path, registry, providers, work) = unheld("codex-unheld-fails");
    std::fs::write(work.join("resume-thread.txt"), "0199c1a1-0000-4000-8000-000000000007").expect("resume answer");
    std::fs::write(work.join("init-hold"), "").expect("hold");
    let runs = codex(&providers).clone();
    let send = |text: &'static str| {
        let (runs, providers, work) = (runs.clone(), providers.clone(), work.to_string_lossy().into_owned());
        tokio::spawn(async move { runs.codex_deliver(providers.gate(), Some(MISMATCHED), Some(&work), text, &SendOptions::default(), None).await })
    };
    let first = send("first");
    wait_for_step(&registry, "codex started", 1).await;
    let second = send("second");
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    std::fs::remove_file(work.join("init-hold")).expect("release");
    assert_eq!(first.await.expect("joined").expect_err("refused").to_string(), "Codex resumed a different thread");
    second.await.expect("joined").expect_err("the message sent meanwhile is answered");
    assert!(runs.codex_handle(MISMATCHED).is_none(), "no handle is left for the session");
    assert!(!registry.host.claim_held(Source::Codex, MISMATCHED), "nor its claim");
}

/// An archive of a session whose app server is still starting is refused
/// at once: waiting for the start would hold the gate as long.
#[tokio::test]
async fn archive_refuses_a_codex_session_still_starting() {
    const RESUMED: &str = "0199c1a1-0000-4000-8000-000000000008";
    let (_home, _path, registry, providers, work) = unheld("codex-unheld-archive");
    std::fs::write(work.join("init-hold"), "").expect("hold");
    let runs = codex(&providers).clone();
    let start = {
        let (runs, providers, work) = (runs.clone(), providers.clone(), work.to_string_lossy().into_owned());
        tokio::spawn(async move { runs.codex_deliver(providers.gate(), Some(RESUMED), Some(&work), "hello", &SendOptions::default(), None).await })
    };
    wait_for_step(&registry, "codex started", 1).await;
    let refused = tokio::time::timeout(Duration::from_secs(1), runs.archive_codex(providers.gate(), RESUMED))
        .await
        .expect("refused at once")
        .expect_err("refused");
    assert!(refused.to_string().contains("still starting"), "{refused}");
    std::fs::remove_file(work.join("init-hold")).expect("release");
    start.await.expect("joined").expect("delivered");
    retire_all(&runs, &[RESUMED]).await;
}

/// A message into a running app server is handed over under the gate and
/// waited for outside it: a driver whose loop is stuck (in a write to its
/// app server, say) answers late, and nothing else waits for that.
#[tokio::test]
async fn a_send_into_a_running_app_server_waits_for_its_answer_outside_the_gate() {
    let (_home, _path, _registry, providers, _work) = unheld("codex-unheld-running");
    let runs = codex(&providers).clone();
    // A driver that takes nothing off its channel.
    let (tx, stuck) = mpsc::unbounded_channel();
    let handle = Handle {
        tx,
        shared: Arc::new(Mutex::new(State::default())),
        account: None,
        starting: Default::default(),
    };
    runs.codex_servers.lock().unwrap().insert("stuck-thread".into(), handle);
    let send = {
        let (runs, providers) = (runs.clone(), providers.clone());
        tokio::spawn(async move { runs.codex_deliver(providers.gate(), Some("stuck-thread"), None, "again", &SendOptions::default(), None).await })
    };
    for _ in 0..500 {
        if !stuck.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!stuck.is_empty(), "the message was handed to the driver");
    drop(tokio::time::timeout(Duration::from_secs(1), providers.gate().permit()).await.expect("the gate is free while the driver answers"));
    assert!(!send.is_finished(), "the send is still waiting for its answer");
    drop(stuck);
    send.await.expect("joined").expect_err("a driver that went away answers nothing");
}

/// A message sent while a session moves to another account waits for the
/// app server it moves to: the old one, retiring, leaves the new one's
/// reservation where it is.
#[tokio::test]
async fn a_send_during_an_account_switch_waits_for_the_new_app_server() {
    let (_home, _path, registry, providers, work) = unheld("codex-unheld-switch");
    let work_ = work.to_string_lossy().into_owned();
    let runs = codex(&providers).clone();
    let (id, _) = runs.codex_deliver(providers.gate(), None, Some(&work_), "hello", &SendOptions::default(), None).await.expect("a new thread");
    for _ in 0..500 {
        if runs.codex_handle(&id).is_some_and(|handle| !handle.busy()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let other = || CodexAccount {
        id: "another".to_string(),
        launch: droppen_accounts::store::AccountLaunch {
            provider: AccountProvider::Codex,
            env: vec![],
            codex_args: vec![],
        },
    };
    std::fs::write(work.join("init-hold"), "").expect("hold");
    let send = |text: &'static str| {
        let (runs, providers, work, id, account) = (runs.clone(), providers.clone(), work_.clone(), id.clone(), other());
        tokio::spawn(async move { runs.codex_deliver(providers.gate(), Some(&id), Some(&work), text, &SendOptions::default(), Some(account)).await })
    };
    let switched = send("switched");
    wait_for_step(&registry, "codex started", 2).await;
    let after = send("after");
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    std::fs::remove_file(work.join("init-hold")).expect("release");
    switched.await.expect("joined").expect("delivered under the other account");
    after.await.expect("joined").expect("delivered into the same app server");
    assert_eq!(registry.host.steps().iter().filter(|step| step.what == "codex started").count(), 2, "the first and the switched one");
    retire_all(&runs, &[&id]).await;
}

/// A session whose app server restarts for a new message, here under
/// another account, reads as working while the new one starts, and still
/// says how its last turn ended: the reserved handle takes that from the
/// one it replaces, until the new app server runs the next turn.
#[tokio::test]
async fn the_last_turn_stays_on_view_while_a_codex_app_server_restarts_for_a_message() {
    let (_home, _path, registry, providers, work) = unheld("codex-unheld-last-turn");
    let work_ = work.to_string_lossy().into_owned();
    let runs = codex(&providers).clone();
    let (id, _) = runs
        .codex_deliver(providers.gate(), None, Some(&work_), "hello", &SendOptions::default(), None)
        .await
        .expect("a new thread");
    for _ in 0..500 {
        if runs.codex_handle(&id).is_some_and(|handle| !handle.busy()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let shown = |injection: Option<Injection>| injection.map(|injection| (injection.state, injection.started_at, injection.finished_at.0));
    let last = shown(providers.injection_for(Source::Codex, &id));
    assert_eq!(last.as_ref().map(|last| last.0), Some(InjectionRunState::Ok), "the first turn ended");

    std::fs::write(work.join("init-hold"), "").expect("hold");
    let other = CodexAccount {
        id: "another".to_string(),
        launch: droppen_accounts::store::AccountLaunch {
            provider: AccountProvider::Codex,
            env: vec![],
            codex_args: vec![],
        },
    };
    let switched = {
        let (runs, providers, work, id) = (runs.clone(), providers.clone(), work_.clone(), id.clone());
        tokio::spawn(async move {
            runs.codex_deliver(providers.gate(), Some(&id), Some(&work), "again", &SendOptions::default(), Some(other))
                .await
        })
    };
    wait_for_step(&registry, "codex started", 2).await;

    let resident = providers.resident_info(Source::Codex, &id).expect("the starting app server is on view");
    assert_eq!(resident.phase, droppen_sessions::ResidentPhase::Starting);
    assert_eq!(
        resident.activity(),
        droppen_sessions::SessionActivity::Working,
        "at work on the message, not idle"
    );
    assert_eq!(shown(providers.injection_for(Source::Codex, &id)), last, "the last turn stays on view");

    std::fs::remove_file(work.join("init-hold")).expect("release");
    switched.await.expect("joined").expect("delivered under the other account");
    retire_all(&runs, &[&id]).await;
}

/// Dropping or steering a queued message of a session whose app server is
/// still starting is refused at once: its driver takes nothing off the
/// queue until codex has answered, and waiting for it would leave the
/// request hanging for the whole start.
#[tokio::test]
async fn queue_actions_on_a_starting_codex_session_are_refused_at_once() {
    const RESUMED: &str = "0199c1a1-0000-4000-8000-000000000009";
    let (_home, _path, registry, providers, work) = unheld("codex-unheld-queue");
    std::fs::write(work.join("init-hold"), "").expect("hold");
    let runs = codex(&providers).clone();
    let start = {
        let (runs, providers, work) = (runs.clone(), providers.clone(), work.to_string_lossy().into_owned());
        tokio::spawn(async move { runs.codex_deliver(providers.gate(), Some(RESUMED), Some(&work), "hello", &SendOptions::default(), None).await })
    };
    wait_for_step(&registry, "codex started", 1).await;
    for steer in [false, true] {
        let outcome = tokio::time::timeout(Duration::from_secs(1), runs.codex_queue_action(RESUMED, "q-1", steer))
            .await
            .expect("refused at once");
        assert!(
            matches!(&outcome, QueueOutcome::Refused { reason } if reason.contains("still starting")),
            "steer {steer}: {outcome:?}"
        );
    }
    std::fs::remove_file(work.join("init-hold")).expect("release");
    start.await.expect("joined").expect("delivered");
    retire_all(&runs, &[RESUMED]).await;
}

/// A message for another account while a session's app server starts is
/// refused at once, as one would be while it runs a turn: the start is the
/// account the session is moving to.
#[tokio::test]
async fn a_send_for_another_account_during_a_start_is_refused() {
    const RESUMED: &str = "0199c1a1-0000-4000-8000-00000000000a";
    let (_home, _path, registry, providers, work) = unheld("codex-unheld-other");
    std::fs::write(work.join("init-hold"), "").expect("hold");
    let runs = codex(&providers).clone();
    let work_ = work.to_string_lossy().into_owned();
    let start = {
        let (runs, providers, work) = (runs.clone(), providers.clone(), work_.clone());
        tokio::spawn(async move { runs.codex_deliver(providers.gate(), Some(RESUMED), Some(&work), "hello", &SendOptions::default(), None).await })
    };
    wait_for_step(&registry, "codex started", 1).await;
    let other = CodexAccount {
        id: "another".to_string(),
        launch: droppen_accounts::store::AccountLaunch {
            provider: AccountProvider::Codex,
            env: vec![],
            codex_args: vec![],
        },
    };
    let refused = tokio::time::timeout(
        Duration::from_secs(1),
        runs.codex_deliver(providers.gate(), Some(RESUMED), Some(&work_), "elsewhere", &SendOptions::default(), Some(other)),
    )
    .await
    .expect("refused at once")
    .expect_err("refused");
    assert_eq!(refused.to_string(), "a message is already running for this session");
    std::fs::remove_file(work.join("init-hold")).expect("release");
    start.await.expect("joined").expect("delivered");
    assert_eq!(registry.host.steps().iter().filter(|step| step.what == "codex started").count(), 1, "nothing else was started");
    retire_all(&runs, &[RESUMED]).await;
}

/// A Stop while a session's app server starts is taken, not refused: it
/// waits behind the message, and the turn the message starts is
/// interrupted as soon as codex says it started. The turn is one the
/// stand-in runs until it is interrupted.
#[tokio::test]
async fn a_stop_while_a_codex_app_server_starts_ends_the_turn_it_starts() {
    const RESUMED: &str = "0199c1a1-0000-4000-8000-00000000000b";
    let (_home, _path, registry, providers, work) = unheld("codex-unheld-stop");
    std::fs::write(work.join("init-hold"), "").expect("hold");
    std::fs::write(work.join("sleep.txt"), "600").expect("a turn that runs until it is stopped");
    let runs = codex(&providers).clone();
    let start = {
        let (runs, providers, work) = (runs.clone(), providers.clone(), work.to_string_lossy().into_owned());
        tokio::spawn(async move {
            runs.codex_deliver(providers.gate(), Some(RESUMED), Some(&work), "hello", &SendOptions::default(), None)
                .await
        })
    };
    wait_for_step(&registry, "codex started", 1).await;
    assert_eq!(runs.codex_handle(RESUMED).expect("reserved").state().phase, Phase::Starting);
    assert!(
        providers.interrupt(Source::Codex, RESUMED),
        "a Stop during the start is taken, not refused"
    );

    std::fs::remove_file(work.join("init-hold")).expect("release");
    start.await.expect("joined").expect("delivered");
    let ended = || {
        providers
            .injection_for(Source::Codex, RESUMED)
            .filter(|injection| injection.state != InjectionRunState::Running)
    };
    for _ in 0..500 {
        if ended().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let ended = ended().expect("the turn ended");
    assert!(ended.interrupted, "the turn ends as stopped: {ended:?}");
    let asked = std::fs::read_to_string(work.join("codex-stdin.jsonl")).unwrap_or_default();
    assert!(asked.contains(r#""method":"turn/interrupt""#), "codex was asked to interrupt it");
    retire_all(&runs, &[RESUMED]).await;
}

/// A session's app server is in the snapshot its driver hands a turn's
/// alert ([`RunHost::driver_states_of`]), as well as in the one the session
/// list asks the providers for.
#[tokio::test]
async fn a_codex_session_is_in_its_turn_alert_s_snapshot() {
    let home = tempfile::tempdir().expect("tempdir");
    let (_path, registry) = fake_codex_registry(home.path(), "codex-snapshot");
    let alerts = Arc::new(droppen_provider::testing::TurnAlerts::default());
    registry.host.attach_alerts(alerts.clone());
    let providers = codex_only(&registry);
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).expect("work");
    let work = work.to_string_lossy().into_owned();
    let (id, _) = codex(&providers)
        .codex_deliver(providers.gate(), None, Some(&work), "hello", &SendOptions::default(), None)
        .await
        .expect("a new thread");
    assert!(providers.driver_states().resident(Source::Codex, &id).is_some(), "in the providers' snapshot");
    let handle = codex(&providers).codex_handle(&id).expect("its app server");
    for _ in 0..500 {
        if !handle.busy() && !alerts.seen().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let seen = alerts.seen();
    assert!(
        seen.iter().any(|(source, alerted, resident)| *source == Source::Codex && *alerted == id && resident.is_some()),
        "the turn's alert saw its app server: {seen:?}"
    );
    retire(&handle).await.expect("retired");
}

/// A thread request's optional members follow the two every thread gets,
/// in this order: the thread being resumed, the provider an account with a
/// key of its own injects, and the chat persona; a chat's turn is
/// read-only. Byte for byte, because the order is on the wire too.
#[tokio::test]
async fn a_resumed_chat_under_an_account_names_its_thread_provider_and_persona_in_order() {
    const THREAD: &str = "0199c1a1-0000-4000-8000-000000000005";
    let home = tempfile::tempdir().expect("tempdir");
    let (_path, registry) = fake_codex_registry(home.path(), "codex-wire");
    let providers = codex_only(&registry);
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).expect("work");
    let log = work.join("codex-stdin.jsonl");
    let work = work.to_string_lossy().into_owned();
    let account = CodexAccount {
        id: "own-key".to_string(),
        launch: droppen_accounts::store::AccountLaunch {
            provider: AccountProvider::Codex,
            env: vec![],
            codex_args: crate::login::CodexBaseUrl::parse("https://api.example.test/v1").expect("a base URL").provider_args(),
        },
    };
    let chat = SendOptions {
        chat: true,
        ..SendOptions::default()
    };
    codex(&providers).codex_deliver(providers.gate(), Some(THREAD), Some(&work), "hi", &chat, Some(account))
        .await
        .expect("resumed");

    let mut lines = Vec::new();
    for _ in 0..500 {
        lines = std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .map(|line| format!("{line}\n"))
            .collect();
        if lines.len() >= 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert_eq!(
        lines[0],
        wire(
            r#"{"id":"UUID","method":"initialize","params":{"clientInfo":{"name":"remote_hand","title":"remote-hand","version":"VERSION"}}}"#,
            &[("UUID", &id_of(&lines[0])), ("VERSION", droppen_http::PACKAGE_VERSION)]
        )
    );
    assert_eq!(lines[1], wire(r#"{"method":"initialized","params":{}}"#, &[]));
    assert_eq!(
        lines[2],
        wire(
            r#"{"id":"UUID","method":"thread/resume","params":{"cwd":"WORK","approvalPolicy":"never","threadId":"THREAD","modelProvider":"remote_hand","baseInstructions":"PERSONA"}}"#,
            &[("UUID", &id_of(&lines[2])), ("WORK", &work), ("THREAD", THREAD), ("PERSONA", CHAT_SYSTEM_PROMPT)]
        )
    );
    let qid = id_of(&lines[3]).strip_prefix("start:").expect("a turn's id").to_owned();
    assert_eq!(
        lines[3],
        wire(
            r#"{"id":"start:QID","method":"turn/start","params":{"input":[{"type":"text","text":"hi"}],"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly"},"threadId":"THREAD","clientUserMessageId":"QID"}}"#,
            &[("QID", &qid), ("THREAD", THREAD)]
        )
    );

    let handle = codex(&providers).codex_handle(THREAD).expect("the resident");
    for _ in 0..500 {
        if !handle.busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    retire(&handle).await.expect("retired");
}

/// A thread request names its directory as a JSON string, which a path
/// that is not UTF-8 cannot be written as: such a session is refused
/// before codex is started, and its claim goes with the refusal. Linux
/// only, because macOS will not create a directory under such a name.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_working_directory_that_is_not_utf8_is_refused_before_codex_starts() {
    use std::os::unix::ffi::OsStrExt;
    const THREAD: &str = "0199c1a1-0000-4000-8000-000000000006";
    let root = tempfile::tempdir().expect("tempdir");
    // The send names no directory, so the session runs in the home.
    let home = root.path().join(std::ffi::OsStr::from_bytes(b"\xff"));
    std::fs::create_dir_all(&home).expect("home");
    let (_path, registry) = fake_codex_registry(&home, "codex-utf8");
    let providers = codex_only(&registry);
    let refused = codex(&providers).codex_deliver(providers.gate(), Some(THREAD), None, "hi", &SendOptions::default(), None)
        .await
        .expect_err("refused");
    assert_eq!(refused.to_string(), "the session's working directory is not valid UTF-8");
    assert_eq!(registry.host.steps(), [step("codex reserved", true, Some(false))], "reserved, and codex was not started");
    assert!(codex(&providers).codex_handle(THREAD).is_none(), "the reservation was given back");
    assert!(!registry.host.gate_held());
    assert!(!registry.host.claim_held(Source::Codex, THREAD));
}

/// A codex session whose app server is gone with messages still queued
/// keeps its handle in the list, and Stop on it stops nothing: the turn was
/// closed when the process went. A live adopted run is put under the same
/// session, the one the fallback would signal, so the test fails if Stop
/// ever gets past the handle.
#[tokio::test]
async fn stop_on_a_codex_session_whose_app_server_is_gone_reaches_no_run() {
    const SESSION: &str = "0199c1a1-6666-7666-8666-dddddddddddd";

    let home = tempfile::tempdir().expect("tempdir");
    let paths = Arc::new(Paths::new(home.path().to_path_buf()));
    let dirs = InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE).expect("valid name");
    dirs.create().expect("instance dirs");
    let decoration = droppen_provider::testing::no_decoration();
    let registry = InjectionRegistry::new(
        paths.clone(),
        &dirs,
        Arc::new(droppen_accounts::store::AccountsStore::new(paths.clone(), logins())),
        Arc::new(droppen_accounts::quota::QuotaCache::new(paths, logins())),
        decoration,
    );
    let providers = codex_only(&registry);

    let mut live = std::process::Command::new("/bin/sh")
        .args(["-c", "sleep 30"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn a stand-in");
    let adopted = runs::RunRecord {
        run_id: "run-adopted".to_string(),
        source: Source::Codex,
        session_id: SESSION.to_string(),
        cwd: None,
        cli_pid: live.id(),
        started_at: "2026-01-01T00:00:00.000Z".to_string(),
        pid_token: procs::start_token(live.id()),
        resident: false,
        shim: None,
        payload: runs::Payload::new(),
    };

    let codex = providers.provider::<Codex>().expect("Codex").runs().clone();
    let app_server = runs::RunRecord {
        run_id: "run-app-server".to_string(),
        resident: true,
        payload: serde_json::from_value(serde_json::json!({"codex": {"queue": [{"id": "q1", "text": "next", "params": {}, "request": null}]}}))
            .expect("a codex state"),
        ..adopted.clone()
    };
    codex.codex_dead(app_server.decode().expect("a Codex record"));
    let handle = codex.codex_handle(SESSION).expect("the handle stays");
    assert!(handle.visible() && !handle.alive(), "listed, with no process behind it");

    let claim = providers.gate().lease(Source::Codex, SESSION).expect("a free session must be claimable");
    registry.watch_adopted(providers.runs()[&Source::Codex].clone(), adopted, claim);

    assert!(!providers.interrupt(Source::Codex, SESSION), "nothing is left to stop");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(live.try_wait().expect("try_wait").is_none(), "Stop got past the handle and signalled the adopted run");
    let seen = registry.host.run_injection(Source::Codex, SESSION).expect("the adopted run");
    assert_eq!(seen.state, InjectionRunState::Running);
    assert!(!seen.interrupted);

    let _ = live.kill();
    let _ = live.wait();
}

/// A Codex row of the run journal, `run_id` and `payload` aside: session
/// `id`, on this process, nothing else known.
fn gone_run(run_id: &str, id: &str, payload: runs::Payload) -> runs::RunRecord {
    runs::RunRecord {
        run_id: run_id.to_string(),
        source: Source::Codex,
        session_id: id.to_string(),
        cwd: None,
        cli_pid: std::process::id(),
        started_at: "2026-01-01T00:00:00.000Z".to_string(),
        pid_token: None,
        resident: false,
        shim: None,
        payload,
    }
}

/// What the base asks Codex's runs of a row: whether they can read it,
/// whether it holds state a dead run must keep (an app server's queue and
/// checkpoint; a `codex exec` run's row holds none), and which CLI to name
/// when it reports on one.
#[tokio::test]
async fn the_runs_say_which_rows_hold_state_and_name_their_cli() {
    let home = tempfile::tempdir().expect("tempdir");
    let registry = droppen_provider::testing::injection_registry(home.path());
    let codex_runs = CodexRuns::new(
        registry.host.clone(),
        registry.decoration.clone(),
        registry.instance.mcp_configs(),
    );
    let exec = gone_run("run-exec", "0199c1a1-9999-7999-8999-dddddddddddd", runs::Payload::new());
    let app_server = gone_run(
        "run-app-server",
        "0199c1a1-9999-7999-8999-dddddddddddd",
        serde_json::from_value(json!({"codex": {"queue": []}})).expect("a codex state"),
    );
    assert!(codex_runs.readable(&exec) && codex_runs.readable(&app_server), "both are Codex's rows");
    assert!(!codex_runs.holds_state(&exec), "a `codex exec` run keeps nothing");
    assert!(codex_runs.holds_state(&app_server), "an app server keeps its queue and checkpoint");
    assert_eq!(codex_runs.cli_name(), "codex");
}

/// What the boot pass asks Codex's runs of a run whose CLI is gone: whether
/// its thread was cut short mid-turn, and when the thread was last
/// written. A thread nothing can find counts as cut short, and so does one
/// whose rollout ends with the user's message; one that ends with Codex's
/// answer does not.
#[tokio::test]
async fn a_gone_runs_thread_says_whether_it_was_cut_short_and_when_it_was_written() {
    const ASKED: &str = "aaaaaaaa-2222-4222-8222-aaaaaaaaaaaa";
    const ANSWERED: &str = "aaaaaaaa-3333-4333-8333-aaaaaaaaaaaa";
    const MISSING: &str = "aaaaaaaa-4444-4444-8444-aaaaaaaaaaaa";
    let _guard = droppen_sessions::index::exclusive().await;
    let home = tempfile::tempdir().expect("tempdir");
    let registry = droppen_provider::testing::injection_registry(home.path());
    let codex_runs = CodexRuns::new(
        registry.host.clone(),
        registry.decoration.clone(),
        registry.instance.mcp_configs(),
    );
    let day = registry.host.paths.codex_sessions().join("2026").join("09").join("21");
    std::fs::create_dir_all(&day).expect("rollout dir");
    for (id, last) in [(ASKED, "user_message"), (ANSWERED, "task_complete")] {
        let meta = json!({"type": "session_meta", "payload": {"cwd": "/w", "id": id}});
        let event = json!({"type": "event_msg", "payload": {"type": last}});
        std::fs::write(day.join(format!("rollout-2026-09-21T10-00-00-{id}.jsonl")), format!("{meta}\n{event}\n")).expect("rollout");
    }
    let gone = |id: &str| gone_run("run-gone", id, runs::Payload::new());
    assert!(codex_runs.was_cut_short(&gone(ASKED)).await, "the user's message was the last thing written");
    assert!(!codex_runs.was_cut_short(&gone(ANSWERED)).await, "Codex had answered");
    assert!(codex_runs.was_cut_short(&gone(MISSING)).await, "a thread that cannot be found is not known to be done");
    assert!(codex_runs.last_write(ASKED).await.is_some(), "its rollout was written");
    assert!(codex_runs.last_write(MISSING).await.is_none());
}

/// An app server that is gone is the session's resident only while it
/// holds messages: with its queue empty there is nothing to show or to
/// decide.
#[tokio::test]
async fn a_gone_app_server_is_a_resident_only_while_it_holds_messages() {
    const KEPT: &str = "0199c1a1-7777-7777-8777-dddddddddddd";
    const EMPTY: &str = "0199c1a1-8888-7888-8888-dddddddddddd";
    let home = tempfile::tempdir().expect("tempdir");
    let registry = droppen_provider::testing::injection_registry(home.path());
    let providers = codex_only(&registry);
    let codex = providers.provider::<Codex>().expect("Codex");
    let dead = |run_id: &str, id: &str, queue: Value| {
        let payload = serde_json::from_value(json!({"codex": {"queue": queue}})).expect("a codex state");
        gone_run(run_id, id, payload).decode().expect("a Codex record")
    };
    codex.runs().codex_dead(dead("run-kept", KEPT, json!([{"id": "q1", "text": "next", "params": {}, "request": null}])));
    codex.runs().codex_dead(dead("run-empty", EMPTY, json!([])));
    assert!(codex.run().resident(KEPT).is_some_and(|state| state.queued == 1), "its message is still there to decide on");
    assert!(codex.run().resident(EMPTY).is_none(), "nothing is left to show");
}

/// A schedule waits for a Codex session's turn: while one runs, the
/// session is busy for it, and once it is over it is not.
#[tokio::test]
async fn a_schedule_waits_while_a_codex_turn_runs() {
    let mut h = Harness::new().await;
    let (handle, _rx) = h.handle();
    h.driver.runs.codex_servers.lock().unwrap().insert("test-thread".into(), handle);
    let providers = h.providers.clone();
    let busy = || providers.provider::<Codex>().expect("Codex").run().schedule_busy("test-thread");
    assert_eq!(busy(), None, "nothing has run yet");
    h.start().await;
    assert_eq!(busy(), Some("the previous run has not finished"));
    h.complete("completed").await;
    assert_eq!(busy(), None, "the turn is over");
}

/// A thread started under a stored account is bound to it as a Codex
/// account, so the next message finds the login this one ran under.
#[tokio::test]
async fn a_thread_started_under_an_account_is_bound_to_it() {
    let home = tempfile::tempdir().expect("tempdir");
    let (_path, registry) = fake_codex_registry(home.path(), "codex-bound");
    let providers = codex_only(&registry);
    let account = registry
        .host
        .accounts
        .create(
            "Relay",
            AccountProvider::Codex,
            droppen_accounts::store::Credential::Env(vec![droppen_base::env_block::EnvVar {
                key: "OPENAI_API_KEY".into(),
                value: "not-a-real-key".into(),
            }]),
        )
        .await
        .expect("a Codex account");
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).expect("work");
    let options = SendOptions {
        account: AccountChoice::Account(account.id.clone()),
        ..SendOptions::default()
    };
    let codex = providers.provider::<Codex>().expect("Codex");
    let at = StartAt {
        cwd: &work.to_string_lossy(),
        text: "hello",
        options: &options,
    };
    let id = codex.run().start(providers.gate(), at).await.expect("a new thread");
    assert_eq!(registry.host.accounts.session_account_id(&id).await, Some(account.id));
}
