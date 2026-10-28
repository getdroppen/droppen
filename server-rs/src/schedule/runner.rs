//! The loop that watches the clock, and everything it does when a task is due.
//!
//! # Where it is started, and why that matters more than it looks
//!
//! [`spawn`] is called from [`crate::app::RemoteHandApp::prepare`] **inside the
//! block that has already proven this process holds the instance lock**, beside
//! the adoptions. Anywhere else and it would run in two servers that must not
//! both act: `export-openapi` builds a throwaway in-process server on a
//! developer's machine, and every integration test builds one per case. A runner
//! outside the lock would have each of those start the user's real nine-o'clock
//! session — a `cargo test` run would spend tokens.
//!
//! # The shape of the loop
//!
//! `interval` + [`MissedTickBehavior::Skip`] + a `select!` on the cancellation
//! token, which is the house pattern [`crate::feed::global`]'s watcher uses,
//! first immediate tick swallowed and all. Five seconds is the grain: the
//! schedules are minute-grained, so the only thing a shorter tick would buy is a
//! more punctual second, and the only thing a longer one would cost is a
//! late-looking task.
//!
//! # Nothing is caught up
//!
//! Two independent guards, for two different gaps:
//!
//! * **the server was down.** [`reschedule_all`] runs once before the loop and
//!   recomputes every enabled task's next occurrence from *now*, throwing away
//!   whatever the file said. A nightly job whose time passed during a restart
//!   waits for tomorrow.
//! * **the machine was asleep.** A due time more than [`LATE_GRACE`] in the past
//!   is recorded as [`TaskStatus::SkippedMissed`] and not run.
//!
//! The reasoning is the same for both, and it is worth stating once: a scheduled
//! prompt is a question about a moment. "Summarise what changed overnight" asked
//! at four in the afternoon is answering about an overnight that has already been
//! worked through, and a weekend's worth of them arriving at once is a dozen CLIs
//! starting together, each spending tokens on a context that has moved on. The
//! honest answer to a missed window is to say it was missed.
//!
//! # `nextRunAt` moves before the task fires
//!
//! Always, and even when the run is then skipped or fails. A run that took longer
//! than the interval between two occurrences would otherwise still be due when
//! the next tick came round, and the task would fire again — and again — while
//! the first was still starting.

use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tokio::time::MissedTickBehavior;

use super::{cron, parse_iso, ScheduledTask, TaskStatus};
use crate::api::AppState;
use crate::inject::{self, SendOptions};
use crate::notify::{self, Level, Nudge};
use crate::openapi_ext::Nullable;
use crate::paths::Paths;
use crate::sessions::{ResidentPhase, Source};
use crate::time::iso8601;

/// How often the clock is consulted.
const TICK: Duration = Duration::from_secs(5);

/// How late a due time may be and still be honoured.
///
/// Generous next to a five-second tick because the loop is not the only thing
/// that can be late: a laptop that suspends for thirty seconds, a machine under
/// a heavy build, a filesystem that stalls. Short next to the schedules it
/// guards, because past this point the run is being made up rather than made.
const LATE_GRACE: Duration = Duration::from_secs(5 * 60);

/// How long scheduler-level alerts are counted over, and how many fit.
///
/// The nudge below goes to [`notify::bark::send`] directly, which means it does
/// **not** pass the per-session limiter on `POST /api/notify` — so it brings its
/// own. Without one, a task on `* * * * *` whose `cwd` has been deleted is a
/// notification a minute, forever, and the user's answer to that is to turn the
/// whole channel off.
const ALERT_WINDOW: Duration = Duration::from_secs(10 * 60);
/// How many scheduler alerts fit inside [`ALERT_WINDOW`], across every task.
///
/// Counted across all of them rather than per task on purpose: five tasks all
/// failing is one situation, and hearing about it five times over does not make
/// it five situations.
const ALERT_BURST: usize = 3;

/// When the recent scheduler alerts went out.
static RECENT_ALERTS: LazyLock<Mutex<Vec<Instant>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// What this tick should do about one task's stored due time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Due {
    /// The time has not come.
    NotYet,
    /// It has, recently enough to honour.
    Fire,
    /// It has, but long enough ago that running now would be making it up.
    Missed,
    /// There is no usable stored time; compute one and wait for it.
    Unscheduled,
}

/// The tick's whole decision, as a function of two instants.
///
/// Pure, so the boundary that decides whether a task runs can be asserted
/// without a clock, a file or a CLI.
fn due_state(next_run_at: Option<SystemTime>, now: SystemTime, grace: Duration) -> Due {
    let Some(at) = next_run_at else { return Due::Unscheduled };
    match now.duration_since(at) {
        // `duration_since` errs when `at` is in the future, which is the common
        // case and not an error here.
        Err(_) => Due::NotYet,
        Ok(late) if late <= grace => Due::Fire,
        Ok(_) => Due::Missed,
    }
}

/// Take a slot for a scheduler alert, or report that the window is full.
fn take_alert_slot(now: Instant) -> bool {
    let mut recent = RECENT_ALERTS.lock().expect("schedule alert limiter poisoned");
    recent.retain(|sent| now.duration_since(*sent) < ALERT_WINDOW);
    if recent.len() >= ALERT_BURST {
        return false;
    }
    recent.push(now);
    true
}

/// Start the loop. Returns immediately; the task stops on the shutdown token.
pub fn spawn(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move { run(state).await });
}

/// Watch the clock until the server stops.
async fn run(state: AppState) {
    let paths = state.paths.clone();
    reschedule_all(&paths, SystemTime::now()).await;

    let mut ticks = tokio::time::interval(TICK);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
    ticks.tick().await; // the first tick is immediate

    loop {
        tokio::select! {
            _ = state.cancel.cancelled() => break,
            _ = ticks.tick() => tick(&state).await,
        }
    }

    tracing::debug!("scheduled task runner stopped");
}

/// Recompute every enabled task's next occurrence from `now`.
///
/// Run once at startup, and the whole of the "no catch-up" promise for the
/// server-was-down case: whatever the file said is discarded rather than
/// compared, so a due time that passed during the downtime cannot be honoured by
/// the first tick.
///
/// It is also where a task with an expression that no longer parses — a
/// hand-edited file, or a croner that has tightened — gets said so, once, rather
/// than every five seconds.
async fn reschedule_all(paths: &Paths, now: SystemTime) {
    let outcome = super::update(paths, |tasks| {
        let mut changed = false;
        for task in tasks.iter_mut() {
            changed |= reschedule(task, now);
        }
        changed.then_some(())
    })
    .await;
    if let Err(error) = outcome {
        tracing::error!(%error, "cannot write the scheduled task list; schedules will be recomputed next boot");
    }
}

/// Put `task.next_run_at` where it belongs for `now`, and say whether that
/// changed anything.
fn reschedule(task: &mut ScheduledTask, now: SystemTime) -> bool {
    let next = task.enabled.then(|| cron::next_occurrence(&task.expr, now)).flatten().map(iso8601);
    // A task whose expression stopped parsing says so once and then stays quiet:
    // the status is only written when it is not already what it would be.
    let mut changed = false;
    if task.enabled && next.is_none() && task.last_status.0 != Some(TaskStatus::InvalidExpression) {
        task.last_run_at = Nullable(Some(iso8601(now)));
        task.last_status = Nullable(Some(TaskStatus::InvalidExpression));
        task.last_error = Nullable(cron::parse(&task.expr).err().or_else(|| Some("the schedule never comes round".into())));
        changed = true;
    }
    if task.next_run_at.0 != next {
        task.next_run_at = Nullable(next);
        changed = true;
    }
    changed
}

/// One pass of the clock: advance everything that came due, then act on it.
///
/// The advance happens first and under the store lock, so what comes back is a
/// list of tasks whose next occurrence is already written down. A slow spawn
/// therefore cannot make the same task due twice, and the runs go out on their
/// own tasks so a slow one cannot hold up the tick behind it.
async fn tick(state: &AppState) {
    let now = SystemTime::now();
    let paths = state.paths.clone();
    let due = super::update(paths.as_ref(), |tasks| {
        let mut due: Vec<ScheduledTask> = Vec::new();
        let mut changed = false;
        for task in tasks.iter_mut() {
            if !task.enabled {
                // A task switched off keeps no schedule, so the UI does not show
                // a time it will not honour.
                if task.next_run_at.0.is_some() {
                    task.next_run_at = Nullable(None);
                    changed = true;
                }
                continue;
            }
            match due_state(task.next_run_at.as_ref().and_then(|at| parse_iso(at)), now, LATE_GRACE) {
                Due::NotYet => {}
                Due::Unscheduled => changed |= reschedule(task, now),
                Due::Fire => {
                    reschedule(task, now);
                    changed = true;
                    due.push(task.clone());
                }
                Due::Missed => {
                    reschedule(task, now);
                    task.last_run_at = Nullable(Some(iso8601(now)));
                    task.last_status = Nullable(Some(TaskStatus::SkippedMissed));
                    task.last_error = Nullable(None);
                    changed = true;
                    tracing::info!(task = %task.name, "a scheduled task's window went by unattended; not making it up");
                }
            }
        }
        changed.then_some(due)
    })
    .await;

    let due = match due {
        Ok(due) => due.unwrap_or_default(),
        Err(error) => {
            // The list could not be written, so nothing may be fired: a run whose
            // advance was not persisted would fire again on the next tick.
            tracing::error!(%error, "cannot write the scheduled task list; skipping this tick");
            return;
        }
    };

    for task in due {
        let state = state.clone();
        tokio::spawn(async move { fire(&state, task).await });
    }
}

/// Start one task's session, or record why it did not start.
async fn fire(state: &AppState, task: ScheduledTask) {
    let paths = state.paths.clone();
    let at = iso8601(SystemTime::now());

    if task.skip_if_busy {
        if let Some(reason) = still_busy(state, &task) {
            tracing::info!(task = %task.name, reason, "a scheduled task's previous session is still working; skipping this run");
            record(&paths, &task.id, |stored| {
                stored.last_run_at = Nullable(Some(at.clone()));
                stored.last_status = Nullable(Some(TaskStatus::SkippedBusy));
                stored.last_error = Nullable(None);
            })
            .await;
            return;
        }
    }

    // Re-checked at the moment of the spawn as well as when the task was saved:
    // a directory that was there in March can be gone in April, and the failure
    // is worth a message rather than an unexplained CLI error.
    let cwd = match crate::files::resolve_safe(&paths, Some(&task.cwd)) {
        Ok(resolved) if tokio::fs::metadata(&resolved).await.map(|meta| meta.is_dir()).unwrap_or(false) => resolved.to_string_lossy().into_owned(),
        _ => {
            failed(state, &task, &at, format!("the working directory is gone: {}", task.cwd)).await;
            return;
        }
    };

    let options = SendOptions {
        model: task.model.0.clone(),
        effort: task.effort.0.clone(),
        permission: task.permission.0.clone(),
        // The same choice `POST /api/sessions/new` makes: without an explicit
        // account a new session follows the default configured for its own CLI;
        // with none, the machine's own login, unless that login is known to be
        // signed out (`accounts::rotation::start_for`).
        use_default_account: true,
        ..Default::default()
    };

    match inject::start_new_session(&state.drive, task.source, &cwd, &task.prompt, &options).await {
        Ok(id) => {
            tracing::info!(task = %task.name, source = %task.source, session = %id, "a scheduled task started a session");
            record(&paths, &task.id, |stored| {
                stored.last_run_at = Nullable(Some(at.clone()));
                stored.last_status = Nullable(Some(TaskStatus::Ok));
                stored.last_error = Nullable(None);
                stored.last_session_id = Nullable(Some(id.clone()));
            })
            .await;
            // The sidebar has to show the new session before its first transcript
            // line lands, so the refresh names it and the feed retries until it
            // appears — exactly as the composer's own new-session route does.
            state.global.request_refresh(Some((task.source, &id)));
        }
        Err(error) => failed(state, &task, &at, error.client_message("could not start the session")).await,
    }
}

/// Whether the session this task last started is still working.
///
/// `None` — not busy — covers every way there is nothing to wait for: the task
/// has never run, or the process it started has since exited.
///
/// The question is the same for both CLIs and the answer comes from different
/// places, because the two run differently rather than because one is
/// half-supported:
///
/// * **claude** keeps a resident between turns, so the honest reading is its
///   phase *and* its queue. A phase that is not `idle` means a turn is in flight
///   or the process is still coming up; a queue that is not empty means work is
///   already waiting behind it, and adding to that is how a backlog starts —
///   remote-hand's queue never refuses a message, it waits, so nothing else
///   would ever apply the brake.
/// * **codex** has no resident and no queue: one process per message, alive
///   exactly while it works. "Is the run still running" is the whole question
///   there, and the injection registry is where it is asked.
fn still_busy(state: &AppState, task: &ScheduledTask) -> Option<&'static str> {
    let previous = task.last_session_id.as_ref()?;
    if task.source == Source::Codex {
        return state
            .drive
            .injections
            .is_running(Source::Codex, previous)
            .then_some("the previous run has not finished");
    }
    let info = state.drive.injections.resident_info(Source::Claude, previous)?;
    if info.phase != ResidentPhase::Idle {
        return Some("a turn is still in flight");
    }
    (info.queued > 0).then_some("messages are still queued")
}

/// Record a failure, and tell the user's phone if there is one to tell.
///
/// Only *this* kind of failure: the run never started, so no turn exists and
/// nothing downstream will ever report it. A session that starts and then goes
/// wrong is already covered — [`crate::push::notify_turn`] fires when its turn
/// ends, exactly as it would for a session started by hand, and for both CLIs.
///
/// The two go out over different channels, and that is worth naming rather than
/// hiding: the turn notification is Web Push to a subscribed browser, while this
/// one is Bark to the phone [`crate::notify`] was configured with. They are not
/// interchangeable, and a user who has set up only one will only hear about the
/// half it covers. Sending this over Web Push instead would be worse: it is
/// addressed to a *session*, and there is no session to address.
async fn failed(state: &AppState, task: &ScheduledTask, at: &str, reason: String) {
    tracing::error!(task = %task.name, reason, "a scheduled task could not start a session");
    record(&state.paths, &task.id, |stored| {
        stored.last_run_at = Nullable(Some(at.to_string()));
        stored.last_status = Nullable(Some(TaskStatus::Failed));
        stored.last_error = Nullable(Some(reason.clone()));
    })
    .await;
    alert(&state.paths, &task.name, &reason).await;
}

/// Put the last run's outcome on the stored task.
async fn record(paths: &Paths, id: &str, patch: impl FnOnce(&mut ScheduledTask)) {
    let outcome = super::update(paths, |tasks| {
        let task = tasks.iter_mut().find(|task| task.id == id)?;
        patch(task);
        Some(())
    })
    .await;
    match outcome {
        Ok(None) => tracing::debug!(task = id, "the scheduled task was deleted while its run was starting"),
        Ok(Some(())) => {}
        Err(error) => tracing::error!(task = id, %error, "cannot record a scheduled task's outcome"),
    }
}

/// Nudge the phone about a run that never started.
///
/// Gated on [`crate::notify::store::StoredNotify::is_live`], so a user who has
/// not configured Bark — or has silenced it — is never the reason a task's
/// failure turns into an HTTP request.
async fn alert(paths: &Paths, task_name: &str, reason: &str) {
    let stored = notify::store::read(paths).await;
    if !stored.is_live() {
        return;
    }
    if !take_alert_slot(Instant::now()) {
        tracing::warn!(task = task_name, "a scheduled task failed, but the alert budget for this window is spent");
        return;
    }
    let nudge = Nudge {
        title: "scheduled task did not run".to_string(),
        body: reason.to_string(),
        subtitle: Some(task_name.to_string()),
        group: Some("Droppen · schedule".to_string()),
        // The settings page is where the task can be fixed, and the only place
        // this failure is visible; a link to a session would be a link to
        // something that was never created.
        url: (!stored.base_url.is_empty()).then(|| format!("{}/settings", stored.base_url)),
        // Not `timeSensitive`: a schedule that missed is worth knowing about at
        // the next glance, not worth breaking a Focus mode for.
        level: Level::Active,
    };
    if let Err(error) = notify::bark::send(&stored.endpoint, &nudge).await {
        tracing::warn!(task = task_name, %error, "could not tell the phone about a failed scheduled task");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(expr: &str, enabled: bool) -> ScheduledTask {
        ScheduledTask {
            id: "a".into(),
            name: "nightly".into(),
            enabled,
            source: Source::Claude,
            expr: expr.into(),
            cwd: "/tmp".into(),
            prompt: "go".into(),
            model: Nullable(None),
            effort: Nullable(None),
            permission: Nullable(None),
            skip_if_busy: true,
            created_at: iso8601(SystemTime::now()),
            updated_at: iso8601(SystemTime::now()),
            next_run_at: Nullable(None),
            last_run_at: Nullable(None),
            last_status: Nullable(None),
            last_error: Nullable(None),
            last_session_id: Nullable(None),
        }
    }

    #[test]
    fn a_time_in_the_future_is_not_yet_due() {
        let now = SystemTime::now();
        assert_eq!(due_state(Some(now + Duration::from_secs(30)), now, LATE_GRACE), Due::NotYet);
    }

    #[test]
    fn a_time_just_gone_fires_and_one_long_gone_does_not() {
        let now = SystemTime::now();
        assert_eq!(due_state(Some(now), now, LATE_GRACE), Due::Fire, "exactly due is due");
        assert_eq!(due_state(Some(now - Duration::from_secs(4)), now, LATE_GRACE), Due::Fire);
        assert_eq!(
            due_state(Some(now - LATE_GRACE), now, LATE_GRACE),
            Due::Fire,
            "the grace itself is inside"
        );
        assert_eq!(
            due_state(Some(now - LATE_GRACE - Duration::from_secs(1)), now, LATE_GRACE),
            Due::Missed,
            "a suspended laptop's backlog is not made up"
        );
        assert_eq!(due_state(Some(now - Duration::from_secs(8 * 3600)), now, LATE_GRACE), Due::Missed);
    }

    #[test]
    fn a_task_with_no_stored_time_is_scheduled_rather_than_fired() {
        assert_eq!(due_state(None, SystemTime::now(), LATE_GRACE), Due::Unscheduled);
    }

    #[test]
    fn rescheduling_an_enabled_task_puts_its_next_run_in_the_future() {
        let now = SystemTime::now();
        let mut task = task("*/5 * * * *", true);
        assert!(reschedule(&mut task, now), "the first schedule is a change");
        let next = parse_iso(task.next_run_at.as_ref().expect("a time")).expect("parse");
        assert!(next > now);
        assert!(
            !reschedule(&mut task, now),
            "the same instant computes the same answer, and writes nothing"
        );
    }

    #[test]
    fn a_disabled_task_keeps_no_schedule() {
        let mut task = task("*/5 * * * *", false);
        task.next_run_at = Nullable(Some(iso8601(SystemTime::now())));
        assert!(reschedule(&mut task, SystemTime::now()));
        assert_eq!(task.next_run_at, Nullable(None));
    }

    #[test]
    fn an_unparseable_expression_is_reported_once() {
        let now = SystemTime::now();
        let mut task = task("not a cron", true);
        assert!(reschedule(&mut task, now));
        assert_eq!(task.last_status, Nullable(Some(TaskStatus::InvalidExpression)));
        assert!(task.last_error.as_ref().is_some_and(|why| !why.is_empty()));
        assert_eq!(task.next_run_at, Nullable(None));
        assert!(
            !reschedule(&mut task, now),
            "saying it again every five seconds would rewrite the file all day"
        );
    }

    #[test]
    fn the_alert_budget_is_spent_and_then_refilled() {
        // A fresh window: the static is process-global, so the test starts far
        // enough back that anything an earlier test left has aged out.
        let base = Instant::now();
        {
            let mut recent = RECENT_ALERTS.lock().expect("limiter");
            recent.clear();
        }
        for _ in 0..ALERT_BURST {
            assert!(take_alert_slot(base));
        }
        assert!(!take_alert_slot(base), "a task failing every minute is not a notification a minute");
        assert!(take_alert_slot(base + ALERT_WINDOW + Duration::from_secs(1)), "the window rolls forward");
    }
}
