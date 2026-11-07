//! The stall watchdog: what it notices, and what it refuses to do about it.
//!
//! Port of the former TypeScript server's `inject-watchdog.test.ts`, and the
//! place where that port stopped being a port. The TypeScript watchdog
//! *reaped*: a run that had produced nothing for twenty minutes was `SIGTERM`ed
//! and reported as stuck. Against a resident claude that is wrong twice over. A
//! session between messages produces nothing by definition, so a reaper would
//! end every idle session on the machine; and a turn that is quiet is not the
//! same as a turn that is dead — a build, a test suite or a background agent
//! can legitimately say nothing for an hour, and killing one destroys work
//! nobody can get back.
//!
//! So the watchdog marks and never kills, and the two halves asserted here are
//! the two halves of that: a long turn that keeps talking finishes normally, and
//! a long turn that says nothing at all is *still running* well past the window
//! that used to end it. Only the user's own Stop ends a turn now, which
//! `tests/inject.rs` covers.
//!
//! `DROPPEN_STALL_MS` and `DROPPEN_WATCHDOG_INTERVAL_MS` are shrunk to
//! milliseconds, which is the same seam the TypeScript test used. They are read once per
//! `InjectionRegistry`, i.e. once per server, so they have to be in place before
//! the first `Probe::start`. This file is its own test binary, so setting them
//! here cannot reach any other test.

mod support;

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::json;
use support::{plan_dripping_cli, plan_slow_cli, Probe};

const SESSION: &str = "cccccccc-3333-4333-8333-cccccccccccc";
const QUIET_SESSION: &str = "cccccccc-3333-4333-8333-dddddddddddd";

/// How long a turn may say nothing before the watchdog calls it stalled.
///
/// Both tests in this binary share it — the registry reads the variable once
/// per server, and these tests run concurrently, so it cannot be set per test.
/// It is measured in seconds rather than the milliseconds the shrink was
/// originally written with because the progressing run drives the watchdog from
/// a `/bin/sh` loop: every tick is a `sleep(1)` fork, and under a loaded CI box
/// (or a `cargo test` using every core) a tick that nominally takes 300 ms can
/// take several times that. A window ten times the tick leaves the test
/// measuring what it is about — whether *progress* keeps a run alive — instead
/// of measuring the scheduler.
const STALL_AFTER: Duration = Duration::from_secs(4);

/// How often the watchdog looks. Small: it is a poll, not a stopwatch, and
/// nothing here asserts on when exactly a quiet turn is noticed.
const WATCHDOG_INTERVAL: Duration = Duration::from_millis(50);

/// How long between two lines of a progressing run's output.
const PROGRESS_TICK: Duration = Duration::from_millis(300);

/// Enough ticks that the run outlives [`STALL_AFTER`] with room to spare, so a
/// wall-clock cap set at the stall window would certainly have ended it.
const PROGRESS_TICKS: u32 = 20;

fn shrink_the_watchdog() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        std::env::set_var("DROPPEN_STALL_MS", STALL_AFTER.as_millis().to_string());
        std::env::set_var("DROPPEN_WATCHDOG_INTERVAL_MS", WATCHDOG_INTERVAL.as_millis().to_string());
    });
}

#[tokio::test]
async fn a_run_that_keeps_making_progress_is_not_killed_however_long_it_takes() {
    shrink_the_watchdog();
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    probe.seed_session(&probe.managed_root(), SESSION, &workspace);

    // A line every tick, for well past the stall window: a wall-clock cap set
    // where the watchdog's is would have ended this run. Ongoing progress has to
    // keep it alive instead.
    plan_dripping_cli(&workspace, PROGRESS_TICKS, PROGRESS_TICK);

    let started = Instant::now();
    probe
        .send_ok("POST", &format!("/api/sessions/claude/{SESSION}/message"), json!({"text": "hello"}), 200)
        .await;

    // The budget is generous on purpose. It exists so a hung server fails the
    // test rather than hanging it; it is not a deadline the run is racing, and
    // a slow machine must not turn "still working" into a failure.
    let injection = probe
        .wait_for_injection_within("claude", SESSION, PROGRESS_TICK * PROGRESS_TICKS * 10 + Duration::from_secs(30))
        .await;
    assert_eq!(injection["state"], "ok", "a healthy finish, got: {injection}");

    // Without this the test could pass vacuously on a machine fast enough to
    // finish the whole run inside the stall window, having proved nothing about
    // the watchdog. The run has to have been alive longer than the window it
    // was never killed by.
    let ran_for = started.elapsed();
    assert!(
        ran_for > STALL_AFTER,
        "the run finished in {ran_for:?}, inside the {STALL_AFTER:?} stall window: \
         nothing was proved about progress keeping it alive"
    );

    probe.stop().await;
}

/// The half that changed: a turn that goes quiet is left alone.
///
/// This is the test that used to assert the opposite — that a silent run was
/// `SIGTERM`ed and reported as stuck — and inverting it is the point of the
/// resident model. The CLI below produces nothing and touches no transcript for
/// far longer than the window, which is exactly what a long tool call, a build,
/// or a background agent looks like from here. Ending it would be indisputably
/// destroying work; the watchdog's whole job is to notice and to leave it.
#[tokio::test]
async fn a_turn_that_stops_making_progress_is_left_running_rather_than_reaped() {
    shrink_the_watchdog();
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    probe.seed_session(&probe.managed_root(), QUIET_SESSION, &workspace);

    // Never writes anything and never touches a transcript: genuinely quiet.
    plan_slow_cli(&workspace, 600.0);

    probe
        .send_ok(
            "POST",
            &format!("/api/sessions/claude/{QUIET_SESSION}/message"),
            json!({"text": "hello"}),
            200,
        )
        .await;
    support::wait_for_prompts(&workspace, 1).await;

    // Several whole windows, with the watchdog polling twenty times a second
    // throughout: an implementation that reaps has had every chance to.
    tokio::time::sleep(STALL_AFTER * 3).await;

    let detail = probe.get_json(&format!("/api/sessions/claude/{QUIET_SESSION}")).await;
    assert_eq!(
        detail["injection"]["state"], "running",
        "a quiet turn is still a turn: {}",
        detail["injection"]
    );
    assert_eq!(detail["injection"]["error"], serde_json::Value::Null);

    // …and the way out is the user's, not a timer's.
    probe
        .send_ok("POST", &format!("/api/sessions/claude/{QUIET_SESSION}/interrupt"), json!({}), 200)
        .await;
    let injection = probe.wait_for_injection_within("claude", QUIET_SESSION, Duration::from_secs(60)).await;
    assert_eq!(injection["state"], "failed");
    assert_eq!(injection["interrupted"], true, "{injection}");

    probe.stop().await;
}
