//! Adoption, the runs map and Stop, over the providers the entry registers.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use droppen_base::procs;
use droppen_instance::instance::InstanceDirs;
use droppen_instance::paths::Paths;
use droppen_sessions::{InjectionRunState, Source};

use crate::provider::codex;
use droppen_provider::*;

/// Stop must never signal a pid that has stopped being the run it named.
///
/// The window this closes is real and routine rather than exotic: an
/// adopted run has no `Child` to be woken by, so [`watch_adopted`] only
/// learns its CLI has exited on a two-second poll, and the kernel is free
/// to hand that pid to anything at all in between. Pressing Stop inside
/// those two seconds used to `SIGTERM` whatever now holds the number.
///
/// The run is put in place through [`InjectionRegistry::resume`] rather
/// than through a boot, because [`InjectionRegistry::adopt_all`] makes the
/// same identity check and would never have adopted this record: the state
/// under test is one that was adopted while its CLI was alive and lost it
/// afterwards, which is the only way a live entry can point at a stranger.
///
/// Stop is pressed through the providers, as the route presses it: claude
/// and codex each fall back to the adopted run when they hold no process
/// for the session.
#[tokio::test]
async fn stop_never_signals_a_pid_that_is_no_longer_the_run_it_named() {
    const CLAUDE: &str = "eeeeeeee-5555-4555-8555-eeeeeeeeeeee";
    const CODEX: &str = "0199c1a1-5555-7555-8555-eeeeeeeeeeee";

    let home = tempfile::tempdir().expect("tempdir");
    let paths = Arc::new(Paths::new(home.path().to_path_buf()));
    let dirs = InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE).expect("valid name");
    let decoration = crate::drive::NotifyDecoration::new(paths.clone(), dirs.clone());
    let registry = InjectionRegistry::new(
        paths.clone(),
        &dirs,
        Arc::new(droppen_accounts::store::AccountsStore::new(paths.clone(), crate::app::logins())),
        Arc::new(droppen_accounts::quota::QuotaCache::new(paths, crate::app::logins())),
        decoration,
    );

    // Alive, and emphatically not an agent run: the developer's `make`,
    // standing in for whatever the recycled pid actually belongs to.
    let mut stranger = std::process::Command::new("/bin/sh")
        .args(["-c", "sleep 30"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn a stand-in");

    let providers = ProviderRegistry::new(registry.clone(), crate::app::register_providers);
    for (source, session) in [(Source::Claude, CLAUDE), (Source::Codex, CODEX)] {
        let claim = providers.gate().lease(source, session).expect("a free session must be claimable");
        droppen_provider::testing::watch_adopted(&registry, 
            providers.runs()[&source].clone(),
            runs::RunRecord {
                run_id: format!("run-{session}"),
                source,
                session_id: session.to_string(),
                cwd: None,
                cli_pid: stranger.id(),
                // A start time no process alive today can have. This is exactly
                // what the record of a run whose pid has been recycled looks
                // like, and the only part of it that can tell.
                started_at: "2026-01-01T00:00:00.000Z".to_string(),
                pid_token: Some("1.000000".to_string()),
                resident: false,
                shim: None,
                payload: runs::Payload::new(),
            },
            claim,
        );
        assert!(
            registry.host.run_injection(source, session).is_some_and(|run| run.state == InjectionRunState::Running),
            "{source}: the run is in flight before Stop"
        );

        assert!(
            !providers.interrupt(source, session),
            "{source}: a pid that fails the identity guard has nothing to interrupt"
        );
    }

    // The assertion the whole fix exists for.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        stranger.try_wait().expect("try_wait").is_none(),
        "a SIGTERM reached a process this server never spawned"
    );

    // And the user is not left with a run that says it is still going and a
    // Stop button that will never end it.
    for (source, session) in [(Source::Claude, CLAUDE), (Source::Codex, CODEX)] {
        let seen = registry.host.run_injection(source, session).expect("the run is still reported");
        assert_eq!(seen.state, InjectionRunState::Ok, "{source}: a run whose pid is gone has ended, not failed");
        assert!(!seen.interrupted, "{source}: nothing was interrupted, so nothing may claim to have been");
        assert!(seen.finished_at.0.is_some(), "{source}: and it has an end time");
    }

    let _ = stranger.kill();
    let _ = stranger.wait();
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
    let decoration = crate::drive::NotifyDecoration::new(paths.clone(), dirs.clone());
    let registry = InjectionRegistry::new(
        paths.clone(),
        &dirs,
        Arc::new(droppen_accounts::store::AccountsStore::new(paths.clone(), crate::app::logins())),
        Arc::new(droppen_accounts::quota::QuotaCache::new(paths, crate::app::logins())),
        decoration,
    );
    let providers = ProviderRegistry::new(registry.clone(), crate::app::register_providers);

    let mut live = std::process::Command::new("/bin/sh")
        .args(["-c", "sleep 30"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
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

    let codex = providers.provider::<codex::app_server::Codex>().expect("Codex").runs().clone();
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
    droppen_provider::testing::watch_adopted(&registry, providers.runs()[&Source::Codex].clone(), adopted, claim);

    assert!(!providers.interrupt(Source::Codex, SESSION), "nothing is left to stop");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(live.try_wait().expect("try_wait").is_none(), "Stop got past the handle and signalled the adopted run");
    let seen = registry.host.run_injection(Source::Codex, SESSION).expect("the adopted run");
    assert_eq!(seen.state, InjectionRunState::Running);
    assert!(!seen.interrupted);

    let _ = live.kill();
    let _ = live.wait();
}

/// Adoption claims each session it takes back without the spawn gate: it
/// runs before the server listens, so no send can race it. A run whose CLI
/// is gone is swept the same way, its claim taken and let go at once, and
/// its lock file goes with it.
#[tokio::test]
async fn adoption_claims_a_live_run_and_sweeps_a_dead_one_without_the_gate() {
    const LIVE: &str = "ffffffff-6666-4666-8666-ffffffffffff";
    const DEAD: &str = "dddddddd-7777-4777-8777-dddddddddddd";

    let home = tempfile::tempdir().expect("tempdir");
    let paths = Arc::new(Paths::new(home.path().to_path_buf()));
    let locks = paths.data_dir().join("inject-locks");
    let debris = locks.join(droppen_sessions::reference::lock_name(Source::Claude.as_str(), DEAD));
    std::fs::create_dir_all(&locks).expect("inject-locks");
    std::fs::write(&debris, b"1\n").expect("the dead run's lock file");
    let dirs = InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE).expect("valid name");
    dirs.create().expect("instance dirs");
    let decoration = crate::drive::NotifyDecoration::new(paths.clone(), dirs.clone());
    let registry = InjectionRegistry::new(
        paths.clone(),
        &dirs,
        Arc::new(droppen_accounts::store::AccountsStore::new(paths.clone(), crate::app::logins())),
        Arc::new(droppen_accounts::quota::QuotaCache::new(paths, crate::app::logins())),
        decoration,
    );
    let providers = ProviderRegistry::new(registry.clone(), crate::app::register_providers);
    let spawn = |script: &str| {
        std::process::Command::new("/bin/sh")
            .args(["-c", script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn a stand-in")
    };
    let mut live = spawn("sleep 30");
    let mut dead = spawn("exit 0");
    dead.wait().expect("it exits");
    let record = |session: &str, pid: u32| runs::RunRecord {
        run_id: format!("run-{session}"),
        source: Source::Claude,
        session_id: session.to_string(),
        cwd: None,
        cli_pid: pid,
        started_at: "2026-01-01T00:00:00.000Z".to_string(),
        pid_token: procs::start_token(pid),
        resident: false,
        shim: None,
        payload: runs::Payload::new(),
    };
    registry.host.journal.add(record(LIVE, live.id()));
    registry.host.journal.add(record(DEAD, dead.id()));

    let adopted = providers.adopt_all().await;
    assert_eq!((adopted.recovered, adopted.finished), (1, 1), "{adopted:?}");
    let step = |what, claim| testing::Step { what, gate: false, claim: Some(claim) };
    assert_eq!(registry.host.steps(), [step("adoption claimed", true), step("buried", false)]);
    assert!(!debris.exists(), "the sweep took the dead run's lock file");
    assert!(registry.host.claim_held(Source::Claude, LIVE), "the watcher holds the claim");

    let _ = live.kill();
    let _ = live.wait();
}

/// The providers over a fresh instance in `home`, as the server builds
/// them, and the injection registry under them.
fn adopting(home: &std::path::Path) -> (Arc<Paths>, Arc<InjectionRegistry>, Arc<ProviderRegistry>) {
    adopting_with(home, crate::app::register_providers)
}

/// [`adopting`], with the providers `register` adds.
fn adopting_with(home: &std::path::Path, register: fn(&mut ProviderRegistry, &Arc<InjectionRegistry>)) -> (Arc<Paths>, Arc<InjectionRegistry>, Arc<ProviderRegistry>) {
    let paths = Arc::new(Paths::new(home.to_path_buf()));
    let dirs = InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE).expect("valid name");
    dirs.create().expect("instance dirs");
    let decoration = crate::drive::NotifyDecoration::new(paths.clone(), dirs.clone());
    let registry = InjectionRegistry::new(
        paths.clone(),
        &dirs,
        Arc::new(droppen_accounts::store::AccountsStore::new(paths.clone(), crate::app::logins())),
        Arc::new(droppen_accounts::quota::QuotaCache::new(paths.clone(), crate::app::logins())),
        decoration,
    );
    let providers = ProviderRegistry::new(registry.clone(), register);
    (paths, registry, providers)
}

/// The pid of a process that has come and gone.
fn gone() -> u32 {
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn a stand-in");
    child.wait().expect("it exits");
    child.id()
}

/// Each run the previous server left is handed to the provider of the CLI
/// that ran it: a Codex app server's to Codex, which keeps its queue and
/// lists it with no process behind it, and a Claude Code run's to Claude
/// Code, whose own transcript says the turn was finished.
#[tokio::test]
async fn adoption_hands_each_run_to_its_own_cli() {
    const CODEX: &str = "0199c1a1-9999-4999-8999-999999999999";
    const CLAUDE: &str = "aaaaaaaa-9999-4999-8999-aaaaaaaaaaaa";
    let home = tempfile::tempdir().expect("tempdir");
    let (paths, registry, providers) = adopting(home.path());

    let codex = serde_json::json!({
        "run_id": "run-codex", "source": "codex", "session_id": CODEX, "cwd": "/work", "cli_pid": gone(),
        "started_at": "2026-01-01T00:00:00.000Z", "pid_token": null, "resident": false, "shim": "shim-codex",
        "fingerprint": null, "turn_at": null, "queued": [],
        "codex": {"active": "turn-1", "queue": [{"id": "m-1", "text": "next", "params": {}, "request": null}]},
    });
    registry.host.journal.insert_raw("run-codex", CODEX, &codex.to_string());
    let claude = serde_json::json!({
        "run_id": "run-claude", "source": "claude", "session_id": CLAUDE, "cwd": "/work", "cli_pid": gone(),
        "started_at": "2026-01-01T00:00:00.000Z",
    });
    registry.host.journal.insert_raw("run-claude", CLAUDE, &claude.to_string());
    let transcript = crate::provider::claude_code::claude_roots::ClaudeRoots::of(&paths).managed_projects().join("-work").join(format!("{CLAUDE}.jsonl"));
    std::fs::create_dir_all(transcript.parent().expect("parent")).expect("project");
    let lines = [
        serde_json::json!({"type": "user", "cwd": "/work", "timestamp": "2026-01-01T00:00:00.000Z", "message": {"content": [{"type": "text", "text": "do the thing"}]}}),
        serde_json::json!({"type": "assistant", "timestamp": "2026-01-01T00:00:05.000Z", "message": {"content": [{"type": "text", "text": "all done"}]}}),
    ];
    std::fs::write(&transcript, lines.map(|line| format!("{line}\n")).concat()).expect("transcript");

    let adopted = providers.adopt_all().await;
    assert_eq!((adopted.recovered, adopted.finished), (0, 2), "{adopted:?}");
    assert!(providers.has_resident(Source::Codex, CODEX), "Codex keeps the app server's queue, listed with no process behind it");
    let kept: Vec<String> = registry.host.journal.read().into_iter().map(|run| run.run_id).collect();
    assert_eq!(kept, ["run-codex"], "Codex keeps its row; Claude Code's finished run leaves none");
    assert!(providers.injection_for(Source::Claude, CLAUDE).is_none(), "Claude Code's transcript says the turn was finished");
}

/// A row whose source this version does not know, or that names none, is
/// adopted by no one: nothing claims its session or buries it, and it
/// stays where it is.
#[tokio::test]
async fn a_row_with_an_unknown_source_or_none_is_left_where_it_is() {
    let home = tempfile::tempdir().expect("tempdir");
    let (_paths, registry, providers) = adopting(home.path());
    for (run, session, source) in [
        ("run-unknown", "bbbbbbbb-9999-4999-8999-bbbbbbbbbbbb", Some("gemini")),
        ("run-nameless", "cccccccc-9999-4999-8999-cccccccccccc", None),
    ] {
        let mut row = serde_json::json!({
            "run_id": run, "session_id": session, "cwd": "/work", "cli_pid": gone(),
            "started_at": "2026-01-01T00:00:00.000Z", "pid_token": null, "resident": false, "shim": null,
            "fingerprint": null, "turn_at": null, "queued": [],
        });
        if let Some(source) = source {
            row["source"] = serde_json::json!(source);
        }
        registry.host.journal.insert_raw(run, session, &row.to_string());
    }

    let adopted = providers.adopt_all().await;
    assert!(adopted.is_empty(), "{adopted:?}");
    assert!(registry.host.steps().is_empty(), "nothing was claimed or buried: {:?}", registry.host.steps());
    assert_eq!(registry.host.journal.raw().len(), 2, "both rows stay as they were");
}

/// A row of a source no provider is registered for is adopted by no one,
/// for want of runs to hand it to: nothing claims its session or buries
/// it, and it stays where it is.
#[tokio::test]
async fn a_row_of_a_source_no_provider_runs_is_left_where_it_is() {
    const CODEX: &str = "0199c1a1-7777-4777-8777-777777777777";
    let home = tempfile::tempdir().expect("tempdir");
    let (_paths, registry, providers) = adopting_with(home.path(), |providers, injections| {
        providers.register(Arc::new(crate::provider::claude_code::resident::ClaudeCode::new(injections.clone())));
    });
    let row = serde_json::json!({
        "run_id": "run-codex", "source": "codex", "session_id": CODEX, "cwd": "/work", "cli_pid": gone(),
        "started_at": "2026-01-01T00:00:00.000Z", "pid_token": null, "resident": false, "shim": null,
        "fingerprint": null, "turn_at": null, "queued": [],
    });
    registry.host.journal.insert_raw("run-codex", CODEX, &row.to_string());

    let adopted = providers.adopt_all().await;
    assert!(adopted.is_empty(), "{adopted:?}");
    assert!(registry.host.steps().is_empty(), "nothing was claimed or buried: {:?}", registry.host.steps());
    assert_eq!(registry.host.journal.raw().len(), 1, "the row stays as it was");
}
