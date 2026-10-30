//! The scheduled-task routes (`droppen_drive::schedule`) on the providers the server
//! registers (`crate::app`).

use std::sync::Arc;

use gotcha::State;

use droppen_http::error::ApiError;
use droppen_http::extract::{ApiJson, ApiPath};
use droppen_instance::paths::Paths;
use droppen_sessions::Source;

use droppen_drive::schedule::{create_scheduled_task, read_tasks, update_scheduled_task, ScheduleState, ScheduledTaskCreate, ScheduledTaskUpdate};

fn paths() -> (tempfile::TempDir, Paths) {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::for_instance(dir.path().to_path_buf(), "default");
    (dir, paths)
}

/// The routes' state over a fresh instance, with the providers `register`
/// adds, and a working directory in its home.
fn state_with(register: fn(&mut droppen_provider::ProviderRegistry, &Arc<droppen_provider::InjectionRegistry>)) -> (tempfile::TempDir, ScheduleState, String) {
    let (dir, paths) = paths();
    let cwd = dir.path().join("w");
    std::fs::create_dir_all(&cwd).expect("cwd");
    (dir, state_on(Arc::new(paths), register), cwd.to_string_lossy().into_owned())
}

/// The routes' state over `paths`, with the providers `register` adds.
fn state_on(paths: Arc<Paths>, register: fn(&mut droppen_provider::ProviderRegistry, &Arc<droppen_provider::InjectionRegistry>)) -> ScheduleState {
    let instance = droppen_instance::instance::InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE).expect("valid name");
    let auth = droppen_auth::Auth::new(&paths, &instance, &droppen_http::config::AuthSettings::default(), false, Arc::new(crate::app::NotifyTokens));
    let drive = droppen_drive::Drive::new(paths.clone(), &instance, auth, crate::app::transcript_sources(), register, crate::app::logins(), crate::app::NotifyDecoration::new(paths.clone(), instance.clone()));
    ScheduleState {
        paths,
        drive,
        global: droppen_drive::feed::global::GlobalFeed::new(),
        cancel: tokio_util::sync::CancellationToken::new(),
    }
}

/// Whether the create route takes a task on `source` with these flags, on a
/// server over `paths` with the providers the entry registers: a fresh one
/// each call, so what a CLI read of `paths` is read again.
async fn flags(paths: &Paths, source: Source, model: Option<&str>, effort: Option<&str>, permission: Option<&str>) -> Result<(), ApiError> {
    let cwd = paths.data_dir().join("w");
    std::fs::create_dir_all(&cwd).expect("cwd");
    let body = serde_json::json!({"source": source, "expr": "0 9 * * *", "cwd": cwd, "prompt": "hi", "model": model, "effort": effort, "permission": permission});
    let body: ScheduledTaskCreate = serde_json::from_value(body).expect("a create body");
    let state = state_on(Arc::new(paths.clone()), crate::app::register_providers);
    create_scheduled_task(State(state), ApiJson(body)).await.map(|_| ())
}

/// The status and message of a refusal.
fn refusal(error: ApiError) -> (u16, String) {
    (error.status().as_u16(), error.body()["error"].as_str().unwrap_or_default().to_string())
}

/// A task runs only on a CLI whose provider takes scheduled tasks. Both
/// routes refuse any other, and store nothing: a stored ACP task would read
/// back as Claude Code and start a run nobody asked for.
#[tokio::test]
async fn neither_route_stores_a_task_on_a_cli_that_runs_none() {
    let (_dir, state, cwd) = state_with(crate::app::register_providers);
    let create = |source: &str| -> ScheduledTaskCreate {
        serde_json::from_value(serde_json::json!({"source": source, "expr": "0 9 * * *", "cwd": cwd, "prompt": "hi"})).expect("a create body")
    };
    let patch = |source: &str| -> ScheduledTaskUpdate { serde_json::from_value(serde_json::json!({"source": source})).expect("a patch body") };
    let refused = (400, "scheduled tasks run on Claude Code or Codex".to_string());

    let error = create_scheduled_task(State(state.clone()), ApiJson(create("acp"))).await.expect_err("refused");
    assert_eq!(refusal(error), refused);
    assert!(read_tasks(&state.paths).await.is_empty(), "nothing was stored");

    let task = create_scheduled_task(State(state.clone()), ApiJson(create("claude"))).await.expect("created").0;
    let error = update_scheduled_task(State(state.clone()), ApiPath(task.id.clone()), ApiJson(patch("acp"))).await.expect_err("refused");
    assert_eq!(refusal(error), refused);
    assert_eq!(read_tasks(&state.paths).await[0].source, Source::Claude, "the task is as it was");

    let moved = update_scheduled_task(State(state.clone()), ApiPath(task.id), ApiJson(patch("codex"))).await.expect("moved").0;
    assert_eq!(moved.source, Source::Codex);
}

#[tokio::test]
async fn each_cli_is_checked_against_its_own_permission_modes() {
    let (_dir, paths) = paths();
    // The values the composer offers for each, and neither CLI's are the
    // other's: `acceptEdits` is not a sandbox and `read-only` is not a
    // permission mode.
    assert!(flags(&paths, Source::Claude, None, None, Some("acceptEdits")).await.is_ok());
    assert!(flags(&paths, Source::Codex, None, None, Some("read-only")).await.is_ok());
    assert!(flags(&paths, Source::Claude, None, None, Some("read-only")).await.is_err());
    assert!(flags(&paths, Source::Codex, None, None, Some("acceptEdits")).await.is_err());
    assert!(flags(&paths, Source::Claude, None, None, Some("nonsense")).await.is_err());
}

#[tokio::test]
async fn a_claude_effort_is_checked_against_the_ladder_the_model_actually_has() {
    let (_dir, paths) = paths();
    assert!(flags(&paths, Source::Claude, Some("opus"), Some("max"), None).await.is_ok());
    assert!(
        flags(&paths, Source::Claude, Some("haiku"), Some("high"), None).await.is_err(),
        "the haiku tier takes no --effort at all"
    );
    assert!(
        flags(&paths, Source::Claude, Some("claude-opus-4-5"), Some("xhigh"), None)
            .await
            .is_err(),
        "that generation predates the rung"
    );
    assert!(flags(&paths, Source::Claude, None, Some("nonsense"), None).await.is_err());
}

#[tokio::test]
async fn a_codex_effort_is_let_through_when_this_machine_has_never_run_codex() {
    let (_dir, paths) = paths();
    // No `models_cache.json`, so nothing here is entitled to an opinion —
    // refusing would hide a rung a new model was released for.
    assert!(flags(&paths, Source::Codex, None, Some("xhigh"), None).await.is_ok());

    // With a cache, the rungs it names are the ones that count.
    std::fs::create_dir_all(paths.codex_dir()).expect("codex dir");
    std::fs::write(
        crate::provider::codex::sessions::codex::CodexHome::of(&paths).models_cache(),
        r#"{"models":[{"slug":"gpt-5.5","visibility":"list","supported_reasoning_levels":[{"effort":"low"},{"effort":"high"}]}]}"#,
    )
    .expect("seed");
    assert!(flags(&paths, Source::Codex, None, Some("high"), None).await.is_ok());
    assert!(flags(&paths, Source::Codex, None, Some("xhigh"), None).await.is_err());
    assert!(
        flags(&paths, Source::Codex, Some("gpt-5.5"), Some("low"), None).await.is_ok(),
        "a named model answers for itself"
    );
}

/// A source no provider runs takes no scheduled tasks either.
#[tokio::test]
async fn a_task_on_a_cli_no_provider_runs_is_refused() {
    let (_dir, state, cwd) = state_with(|_, _| {});
    for source in [Source::Claude, Source::Codex, Source::Acp] {
        let body: ScheduledTaskCreate =
            serde_json::from_value(serde_json::json!({"source": source, "expr": "0 9 * * *", "cwd": cwd, "prompt": "hi"})).expect("a create body");
        let error = create_scheduled_task(State(state.clone()), ApiJson(body)).await.expect_err("refused");
        assert_eq!(refusal(error), (400, "scheduled tasks run on Claude Code or Codex".to_string()), "{source}");
    }
}
