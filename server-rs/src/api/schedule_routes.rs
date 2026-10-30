//! The scheduled-task routes (`crate::schedule`) on the providers the server
//! registers (`crate::app`).

use std::sync::Arc;

use gotcha::State;

use droppen_http::error::ApiError;
use droppen_http::extract::{ApiJson, ApiPath};
use droppen_instance::paths::Paths;
use droppen_sessions::Source;

use crate::schedule::{create_scheduled_task, read_tasks, update_scheduled_task, ScheduleState, ScheduledTaskCreate, ScheduledTaskUpdate};

fn paths() -> (tempfile::TempDir, Paths) {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::for_instance(dir.path().to_path_buf(), "default");
    (dir, paths)
}

/// The routes' state over a fresh instance, with the providers `register`
/// adds, and a working directory in its home.
fn state_with(register: fn(&mut crate::provider::ProviderRegistry, &Arc<crate::provider::InjectionRegistry>)) -> (tempfile::TempDir, ScheduleState, String) {
    let (dir, paths) = paths();
    let paths = Arc::new(paths);
    let cwd = dir.path().join("w");
    std::fs::create_dir_all(&cwd).expect("cwd");
    let instance = droppen_instance::instance::InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE).expect("valid name");
    let auth = droppen_auth::Auth::new(&paths, &instance, &droppen_http::config::AuthSettings::default(), false, Arc::new(crate::app::NotifyTokens));
    let drive = crate::drive::Drive::new(paths.clone(), &instance, auth, crate::app::transcript_sources(), register, crate::app::logins());
    let state = ScheduleState {
        paths,
        drive,
        global: crate::feed::global::GlobalFeed::new(),
        cancel: tokio_util::sync::CancellationToken::new(),
    };
    (dir, state, cwd.to_string_lossy().into_owned())
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
