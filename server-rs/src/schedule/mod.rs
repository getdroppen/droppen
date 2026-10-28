//! Scheduled tasks: a cron expression, a prompt, and a directory to run it in.
//!
//! The whole feature is one sentence — *at nine every morning, start a session
//! in this repository with this prompt* — and most of the design work went into
//! what it deliberately does **not** do.
//!
//! # It only ever starts a new session
//!
//! The obvious second half is "and send this to an existing session", and it is
//! absent on purpose. A send has to pass [`crate::sessions::sendability`], find
//! the session still writable, and either reach a resident or respawn one with a
//! matching fingerprint; every one of those is a decision a *person* was making
//! when the send was a button, and a timer that makes them unattended has no way
//! to be told it guessed wrong. Starting a new session has none of that: the
//! session does not exist yet, so nothing about it can be in the wrong state.
//! Injection into an existing session is a later version's problem.
//!
//! Both CLIs, though: which one a task runs is a field on it, because
//! [`crate::inject::start_new_session`] already takes a [`Source`] and there is
//! nothing about a timer that prefers one. The two differ in three places, all of
//! them handled rather than papered over — the flag values each accepts (see
//! [`crate::models::accepted_efforts`] and
//! [`crate::models::permission_is_known`]), what "still busy" means (see
//! [`runner`]), and the fact that only a claude spawn is decorated with the
//! `rh-notify` skill, so a codex session started by a task cannot nudge a phone
//! from inside itself. That last one is Claude Code's own mechanism and not
//! something this feature can lend to codex; the scheduler's *own* failures are
//! reported either way.
//!
//! # Missed runs are not made up
//!
//! A task whose time passed while the server was down does not run at startup.
//! The runner recomputes every schedule from the current time and lets the gap
//! go. Two reasons, and they are the same reason twice: a nine-in-the-morning
//! review that arrives at four in the afternoon is answering a question about a
//! morning that is over, and a machine that was asleep for a weekend would come
//! back to a burst of CLIs all starting at once, each spending tokens on a
//! context that has moved on. See [`runner`].
//!
//! # Layout
//!
//! * [`cron`] — the expression, and the one question the runner asks it;
//! * [`runner`] — the five-second loop, and everything it does when a task is due;
//! * this file — the `0600` store and the four CRUD routes over it.
//!
//! # Why the list is this instance's own
//!
//! `<data>/instances/<name>/state.db`, this instance's own database. A shared
//! list would not merely be confusing: every server running on the machine
//! would read the same list and every one of them would fire it, so two
//! instances would mean two sessions per tick, each unaware of the other's.
//! The task list is a thing a server *acts on*, which makes it ownership state,
//! and ownership state belongs to the instance that holds the lock.
//!
//! `0600`, because a prompt is content the user wrote and a `cwd` names their
//! source tree. That rule moved with the tasks: the database is kept `0600` by
//! `Db::keep_to_ourselves`, and nothing here writes a plaintext copy beside it
//! any more.
//!
//! # Where the tasks actually are
//!
//! One row each in [`crate::db`]'s `scheduled_tasks`, in this instance's state
//! database. `scheduled-tasks.json` was written for a while after the move, so
//! that a [`crate::supervisor`] demotion to a `.prev` binary — one that had
//! never heard of the database — would not come back having forgotten every
//! task, which is a silent feature outage. No binary that old is left to
//! demote to, and the file is no longer read: one left on disk is inert.

pub mod cron;
pub mod runner;

use std::sync::Arc;
use std::time::SystemTime;

use gotcha::{api, Json, Schematic, State};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::error::ApiError;
use crate::api::extract::{ApiJson, ApiPath};
use crate::api::{ApiRouter, AppState, SendResult};
use crate::db::Db;
use crate::openapi_ext::{ExtraResponses, Nullable, Optional};
use crate::sessions::{iso8601, Paths, Source};

/// How long a task's display name may be, in characters.
const MAX_NAME: usize = 100;

/// How many tasks one instance may hold.
///
/// A bound rather than a policy: the runner re-reads the list every five
/// seconds, and a list nobody capped is a list one buggy client can grow until
/// that read is no longer free. Fifty is far past what a person schedules by
/// hand.
///
/// Enforced on the way *in* and not on the way out. Truncating an over-long file
/// on read would be silent data loss the moment anything wrote it back, and this
/// is a cap on what the API will create rather than a claim about what the file
/// may contain.
const MAX_TASKS: usize = 50;

/// Serializes read-modify-write across every caller in the process, for the
/// reason [`crate::notify::store`]'s does, and it outlived the file it was
/// introduced for: [`update`] reads the whole list, hands it to a closure and
/// writes it all back, so the read and the write are in different transactions.
/// The runner advances `nextRunAt` on its own task while a route is editing the
/// same list, and two of those interleaved would have one of them write back a
/// copy that predates the other's change.
static WRITE_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/* ── the wire shapes ─────────────────────────────────────────────────── */

/// What became of a task the last time the scheduler acted on it.
///
/// An enum rather than the free text it started as, and the deviation is the
/// point: the web draws a coloured dot from this and a translated line beside
/// it, and a string like `"skipped (busy)"` can be neither coloured nor
/// translated. What the *user* needs to read lives in `lastError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    /// A session was started. Its id is in `lastSessionId`.
    Ok,
    /// The previous session was still working, and `skipIfBusy` is on.
    SkippedBusy,
    /// The due time went by while nothing was running it — a suspended laptop,
    /// most often. Deliberately not made up afterwards; see the module docs.
    SkippedMissed,
    /// The CLI could not be started at all. `lastError` says why.
    Failed,
    /// The stored expression does not parse, so the task can never fire.
    InvalidExpression,
}

// None of the [`Nullable`] fields below carries `#[serde(default)]`, unlike the
// sibling stores': gotcha's derive reads serde's attributes, and one there makes
// the field *optional* as well as nullable — `string | null | undefined` in the
// generated TypeScript, for fields that are always written. Nothing needs the
// default: this type is only ever deserialized field by field, by `read_tasks`.
/// One scheduled task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTask {
    /// Stable id, a uuid v4.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Whether the runner looks at it at all.
    pub enabled: bool,
    /// Which CLI the run starts.
    pub source: Source,
    /// The five-field cron expression, read in the server's local time zone.
    /// See [`cron`] for why that zone and not UTC.
    pub expr: String,
    /// Working directory the session starts in, already resolved and checked to
    /// be a directory when it was stored.
    pub cwd: String,
    /// The first — and, for this feature, only — prompt.
    pub prompt: String,
    /// `--model`, or `null` for the CLI's own default.
    pub model: Nullable<String>,
    /// Reasoning effort, or `null`.
    pub effort: Nullable<String>,
    /// Permission / sandbox mode, or `null`.
    pub permission: Nullable<String>,
    /// Skip this run when the session the last one made is still busy.
    ///
    /// On by default. remote-hand's queue never refuses a message — it waits — so
    /// without this a half-hourly task whose sessions take forty minutes would
    /// quietly build a backlog that only ever grows.
    pub skip_if_busy: bool,
    /// When it was created, ISO-8601.
    pub created_at: String,
    /// When it last changed, ISO-8601.
    pub updated_at: String,
    /// When it next fires, ISO-8601, or `null` while it is disabled or unusable.
    pub next_run_at: Nullable<String>,
    /// When the scheduler last acted on it, ISO-8601 — which is not the same as
    /// "last ran": a skip is an action and sets this too.
    pub last_run_at: Nullable<String>,
    /// What that action was.
    pub last_status: Nullable<TaskStatus>,
    /// The detail behind a `failed` or `invalidExpression`, for a person to read.
    pub last_error: Nullable<String>,
    /// The session the last successful run started, and the one `skipIfBusy`
    /// asks about.
    pub last_session_id: Nullable<String>,
}

/// The scheduled-task list response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
pub struct ScheduledTaskList {
    /// Every task, in file order.
    pub tasks: Vec<ScheduledTask>,
}

/// The body of `POST /api/scheduled-tasks`.
///
/// The four required fields are the ones a task cannot be invented without; the
/// rest are [`Optional`] rather than `Option` throughout, because the contract is
/// zod's `.optional()` and a plain `Option` would document `T | null`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTaskCreate {
    /// Display name; blank or missing becomes `task N`.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub name: Optional<String>,
    /// Which CLI to start.
    pub source: Source,
    /// The five-field cron expression.
    pub expr: String,
    /// Working directory; must exist and be under the home directory.
    pub cwd: String,
    /// The prompt the session starts with.
    pub prompt: String,
    /// Whether it starts enabled. Defaults to `true`: a task nobody asked to be
    /// dormant was created to run.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub enabled: Optional<bool>,
    /// `--model`.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub model: Optional<String>,
    /// Reasoning effort.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub effort: Optional<String>,
    /// Permission / sandbox mode.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub permission: Optional<String>,
    /// Skip a run whose predecessor is still busy. Defaults to `true`.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub skip_if_busy: Optional<bool>,
}

/// The body of `PATCH /api/scheduled-tasks/{id}`; every field is a patch.
///
/// An empty string in `model`, `effort` or `permission` clears it back to the
/// CLI's own default — the one thing an absent field cannot express, and the
/// only way a user who set a model once can unset it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTaskUpdate {
    /// New display name; a blank one is ignored.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub name: Optional<String>,
    /// Switch which CLI the run starts. The flags are re-checked against it, so
    /// a `permission` the other CLI has never heard of is refused here rather
    /// than at the spawn.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub source: Optional<Source>,
    /// New cron expression. Reschedules from now.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub expr: Optional<String>,
    /// New working directory; checked exactly as on create.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub cwd: Optional<String>,
    /// New prompt.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub prompt: Optional<String>,
    /// Start or stop looking at it. Enabling reschedules from now, so a task
    /// switched back on does not immediately fire for a time that has passed.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub enabled: Optional<bool>,
    /// `--model`; `""` clears it.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub model: Optional<String>,
    /// Reasoning effort; `""` clears it.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub effort: Optional<String>,
    /// Permission / sandbox mode; `""` clears it.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub permission: Optional<String>,
    /// Whether to skip a run whose predecessor is still busy.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub skip_if_busy: Optional<bool>,
}

/* ── the store ───────────────────────────────────────────────────────── */

/// A fresh uuid v4.
fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Now, in the format every other timestamp in this server uses.
fn now_iso() -> String {
    iso8601(SystemTime::now())
}

/// A stored ISO-8601 stamp as an instant, if it is one.
///
/// The parser lives in [`crate::time::parse_iso`], beside the clock; it stays
/// re-exported here because the agent's stream code and the aggregator reach
/// for it under this path.
pub use crate::time::parse_iso;

/// A string field, or the empty string, from a hand-editable file.
fn string_at(entry: &Value, key: &str) -> String {
    entry.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// An optional string field: absent, `null` and the wrong type all read as none.
fn optional_at(entry: &Value, key: &str) -> Nullable<String> {
    Nullable(
        entry
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
    )
}

/// One task, rebuilt field by field from whatever the store handed back.
///
/// Per-field coercion rather than one `serde_json::from_str`, for the reason
/// [`crate::notify::store::read`] gives: a row that has been damaged — by an
/// older build, or by hand — with a number where a string belongs should cost
/// that field and not the whole list. The `payload` column *is* the JSON a
/// [`ScheduledTask`] serialises to, which is the shape the entries have always
/// had.
fn task_at(entry: &Value, index: usize) -> ScheduledTask {
    ScheduledTask {
        id: entry
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map_or_else(new_id, str::to_owned),
        name: sanitize_name(entry.get("name").and_then(Value::as_str), &format!("task {}", index + 1)),
        // A task whose `enabled` went missing is off: the failure mode of
        // guessing wrong in the other direction is a CLI nobody asked for.
        enabled: entry.get("enabled").and_then(Value::as_bool).unwrap_or(false),
        // Claude when the file does not say, which is what a file written
        // before this field existed means.
        source: match entry.get("source").and_then(Value::as_str) {
            Some("codex") => Source::Codex,
            _ => Source::Claude,
        },
        expr: string_at(entry, "expr"),
        cwd: string_at(entry, "cwd"),
        prompt: string_at(entry, "prompt"),
        model: optional_at(entry, "model"),
        effort: optional_at(entry, "effort"),
        permission: optional_at(entry, "permission"),
        skip_if_busy: entry.get("skipIfBusy").and_then(Value::as_bool).unwrap_or(true),
        created_at: entry.get("createdAt").and_then(Value::as_str).map_or_else(now_iso, str::to_owned),
        updated_at: entry.get("updatedAt").and_then(Value::as_str).map_or_else(now_iso, str::to_owned),
        next_run_at: optional_at(entry, "nextRunAt"),
        last_run_at: optional_at(entry, "lastRunAt"),
        last_status: Nullable(
            entry
                .get("lastStatus")
                .and_then(|value| serde_json::from_value::<TaskStatus>(value.clone()).ok()),
        ),
        last_error: optional_at(entry, "lastError"),
        last_session_id: optional_at(entry, "lastSessionId"),
    }
}

/// A task as the `payload` column holds it.
fn payload(task: &ScheduledTask) -> String {
    serde_json::to_string(task).unwrap_or_else(|_| "{}".to_string())
}

/// This instance's database.
fn store(paths: &Paths) -> Arc<Db> {
    crate::db::at(paths.instance_db())
}

/// Every saved task, in the order they were created in.
///
/// `ORDER BY rowid` rather than a column, because [`write_tasks`] replaces the
/// whole table in one transaction and so the rowids follow the list it was
/// handed. A row that will not parse is dropped rather than fatal.
pub async fn read_tasks(paths: &Paths) -> Vec<ScheduledTask> {
    let db = store(paths);
    db.read(|conn| {
        let mut statement = conn.prepare("SELECT payload FROM scheduled_tasks ORDER BY rowid")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows
            .filter_map(Result::ok)
            .filter_map(|payload| serde_json::from_str::<Value>(&payload).ok())
            .enumerate()
            .map(|(index, entry)| task_at(&entry, index))
            .collect())
    })
    .unwrap_or_default()
}

/// Replace the stored list with `tasks`.
///
/// The transaction is the whole of the save; nothing re-encodes
/// `scheduled-tasks.json` afterwards, because nothing reads it. The `io::Result`
/// is therefore the transaction's alone — a database that will not commit is a
/// save the user has to be told about, and the routes turn it into a `500`.
pub async fn write_tasks(paths: &Paths, tasks: &[ScheduledTask]) -> std::io::Result<()> {
    let db = store(paths);
    db.write(|tx| {
        // Replace rather than merge: the list handed in is the whole truth, and
        // a task the user deleted must not survive as a row the runner still
        // fires.
        tx.execute("DELETE FROM scheduled_tasks", [])?;
        for task in tasks {
            // `OR REPLACE` so two tasks that somehow share an id cost one task
            // rather than the whole save — including the `nextRunAt` advance the
            // runner is in the middle of, which must never fail on a duplicate.
            tx.execute(
                "INSERT OR REPLACE INTO scheduled_tasks (id, payload) VALUES (?1, ?2)",
                (&task.id, payload(task)),
            )?;
        }
        Ok(())
    })
    .map_err(std::io::Error::other)?;
    Ok(())
}

/// Read, change and write the list under the lock.
///
/// The closure answers `None` for "nothing to do", and the elision that answer
/// buys is worth restating now that the mirror is gone, because half of its
/// reason went with it. It used to save a whole-file encode, a `0600` create, an
/// `fsync` and a rename of `scheduled-tasks.json` — 17 280 of them a day, at the
/// runner's five-second tick, all of them saying nothing had changed. Against a
/// table that is not the shape of the cost: an `UPDATE` that changes nothing is
/// very nearly free.
///
/// It stays because "very nearly free" is not free. The idle tick is by far the
/// commonest way through here, and eliding it keeps the scheduler from opening,
/// committing and `fsync`ing a WAL transaction seventeen thousand times a day to
/// rewrite every row with the bytes it already had. That is now a nicety rather
/// than the load-bearing thing it was; it is also two lines.
pub async fn update<T>(paths: &Paths, patch: impl FnOnce(&mut Vec<ScheduledTask>) -> Option<T>) -> std::io::Result<Option<T>> {
    let _guard = WRITE_LOCK.lock().await;
    let mut tasks = read_tasks(paths).await;
    let Some(outcome) = patch(&mut tasks) else {
        return Ok(None);
    };
    write_tasks(paths, &tasks).await?;
    Ok(Some(outcome))
}

/* ── validation ──────────────────────────────────────────────────────── */

/// A trimmed, length-capped name, or `fallback` when nothing usable was given.
fn sanitize_name(value: Option<&str>, fallback: &str) -> String {
    let Some(value) = value else { return fallback.to_string() };
    let name: String = value.trim().chars().take(MAX_NAME).collect();
    if name.is_empty() {
        fallback.to_string()
    } else {
        name
    }
}

/// The directory a task's sessions start in, resolved and proven to exist.
///
/// Checked here, when the task is written, rather than when it fires: a timer
/// that discovers at three in the morning that its `cwd` was a typo has nobody
/// to tell, and the person who could fix it is asleep. The same two checks
/// `POST /api/sessions/new` makes, in the same order.
async fn validated_cwd(paths: &Paths, cwd: &str) -> Result<String, ApiError> {
    let resolved = crate::files::resolve_safe(paths, Some(cwd)).map_err(|error| error.into_api("invalid working directory"))?;
    if !tokio::fs::metadata(&resolved).await.map(|meta| meta.is_dir()).unwrap_or(false) {
        return Err(ApiError::bad_request(format!("not a directory: {}", resolved.display())));
    }
    Ok(resolved.to_string_lossy().into_owned())
}

/// The flag values, checked now rather than at the moment of the spawn, and
/// checked against the CLI that will actually be handed them.
///
/// Two layers, and they answer different questions:
///
/// 1. **shape** — the composer's own check
///    ([`crate::drive::routes::validate_flags`]), reused rather than restated so
///    the two surfaces cannot drift. These values become `argv`, and this is what
///    stops a `model` from reading as a flag. Identical for both CLIs, because
///    the hazard is.
/// 2. **meaning** — whether *this* CLI takes that effort and that permission
///    mode. Not identical, and not something one rule can cover: claude's efforts
///    are a ladder that narrows per model and its permission modes are its own
///    five, while codex reads its rungs out of the cache the CLI writes and
///    names its sandbox modes differently. See [`crate::models::accepted_efforts`]
///    for why a codex machine with no cache is let through.
///
/// The second layer is what this feature is *for*. The composer has a person in
/// front of it who can read a refusal and try again; a task at three in the
/// morning has nobody, so the moment to refuse is while they are still looking at
/// the form. The `model` itself is deliberately left to the shape check: both
/// model lists are open by construction — claude's concrete ids come out of usage
/// history, codex's out of a cache — so a model this machine has not run yet is
/// not a model that does not exist.
async fn validated_flags(paths: &Paths, source: Source, model: Option<&str>, effort: Option<&str>, permission: Option<&str>) -> Result<(), ApiError> {
    if source == Source::Acp {
        return Err(ApiError::bad_request("scheduled tasks run on Claude Code or Codex"));
    }
    crate::drive::routes::validate_flags(model, effort, permission).map_err(ApiError::bad_request)?;

    if let Some(effort) = effort.filter(|effort| !effort.is_empty()) {
        if let Some(accepted) = crate::models::accepted_efforts(paths, source, model).await {
            if !accepted.iter().any(|known| known == effort) {
                return Err(ApiError::bad_request(match accepted.is_empty() {
                    true => format!("{source} takes no effort for that model"),
                    false => format!("unknown {source} effort: {effort} (expected one of {})", accepted.join(", ")),
                }));
            }
        }
    }

    if let Some(permission) = permission.filter(|permission| !permission.is_empty()) {
        if !crate::models::permission_is_known(source, permission) {
            return Err(ApiError::bad_request(format!("unknown {source} permission mode: {permission}")));
        }
    }
    Ok(())
}

/// The expression, checked now, for the same reason.
fn validated_expr(expr: &str) -> Result<String, ApiError> {
    cron::parse(expr).map_err(ApiError::bad_request)?;
    Ok(expr.trim().to_string())
}

/// A supplied flag value, trimmed, with blank meaning "no value".
///
/// The one normalization both routes go through before anything else looks at
/// these, and it has to be first: `validate_flags` matches `model` against a
/// pattern that an empty string fails, so `{"model": ""}` would otherwise be a
/// confusing `400` on create while meaning "clear it" on update. Blank is the
/// only spelling a form has for "I do not want to set this", and it should mean
/// the same thing on both routes.
fn flag(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|value| !value.is_empty()).map(str::to_owned)
}

/// An empty patch string clears the flag; an absent one leaves it alone.
fn patched_flag(current: &Nullable<String>, patch: Option<&str>) -> Nullable<String> {
    match patch {
        None => current.clone(),
        Some(value) => Nullable(flag(Some(value))),
    }
}

/// A write failure the client sees as a `500`.
fn write_failed(error: std::io::Error) -> ApiError {
    ApiError::internal(format!("failed to write scheduled tasks: {error}"))
}

/* ── routes ──────────────────────────────────────────────────────────── */

/// Every scheduled task this instance runs.
#[api(id = "list_scheduled_tasks", group = "schedule", summary = "List scheduled tasks")]
pub async fn list_scheduled_tasks(State(state): State<AppState>) -> Json<ScheduledTaskList> {
    Json(ScheduledTaskList {
        tasks: read_tasks(state.paths.as_ref()).await,
    })
}

/// Create a scheduled task.
#[api(id = "create_scheduled_task", group = "schedule", summary = "Create a scheduled task")]
pub async fn create_scheduled_task(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<ScheduledTaskCreate>,
) -> Result<Json<ScheduledTask>, ApiError> {
    let paths = state.paths.as_ref();
    let expr = validated_expr(&body.expr)?;
    crate::drive::routes::validate_text(&body.prompt).map_err(ApiError::bad_request)?;
    let (model, effort, permission) = (
        flag(body.model.as_deref()),
        flag(body.effort.as_deref()),
        flag(body.permission.as_deref()),
    );
    validated_flags(paths, body.source, model.as_deref(), effort.as_deref(), permission.as_deref()).await?;
    let cwd = validated_cwd(paths, &body.cwd).await?;

    let enabled = body.enabled.0.unwrap_or(true);
    let stamp = now_iso();
    let created = update(paths, |tasks| {
        if tasks.len() >= MAX_TASKS {
            return None;
        }
        let task = ScheduledTask {
            id: new_id(),
            name: sanitize_name(body.name.as_deref(), &format!("task {}", tasks.len() + 1)),
            enabled,
            source: body.source,
            next_run_at: Nullable(enabled.then(|| cron::next_occurrence(&expr, SystemTime::now())).flatten().map(iso8601)),
            expr,
            cwd,
            prompt: body.prompt,
            model: Nullable(model),
            effort: Nullable(effort),
            permission: Nullable(permission),
            skip_if_busy: body.skip_if_busy.0.unwrap_or(true),
            created_at: stamp.clone(),
            updated_at: stamp,
            last_run_at: Nullable(None),
            last_status: Nullable(None),
            last_error: Nullable(None),
            last_session_id: Nullable(None),
        };
        tasks.push(task.clone());
        Some(task)
    })
    .await
    .map_err(write_failed)?;

    Ok(Json(
        created.ok_or_else(|| ApiError::conflict(format!("at most {MAX_TASKS} scheduled tasks")))?,
    ))
}

/// Change a scheduled task. Absent fields are left alone.
#[api(id = "update_scheduled_task", group = "schedule", summary = "Update a scheduled task")]
pub async fn update_scheduled_task(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<ScheduledTaskUpdate>,
) -> Result<Json<ScheduledTask>, ApiError> {
    let paths = state.paths.as_ref();
    let expr = body.expr.as_deref().map(validated_expr).transpose()?;
    if let Some(prompt) = body.prompt.as_deref() {
        crate::drive::routes::validate_text(prompt).map_err(ApiError::bad_request)?;
    }
    // Read before the lock, because what the flags have to be checked against is
    // the task *as it will be*: a patch that only switches `source` still has to
    // prove the effort and permission mode already stored are ones the other CLI
    // takes. The checks are async (codex's rungs come off the disk), so they
    // cannot run inside the closure below; the values they vouch for are
    // re-derived there from whatever is current, and each of those is either this
    // patch's — just checked — or one that was checked when it was set.
    let stored = read_tasks(paths)
        .await
        .into_iter()
        .find(|task| task.id == id)
        .ok_or_else(|| ApiError::not_found("scheduled task not found"))?;
    let source = body.source.0.unwrap_or(stored.source);
    validated_flags(
        paths,
        source,
        patched_flag(&stored.model, body.model.as_deref()).0.as_deref(),
        patched_flag(&stored.effort, body.effort.as_deref()).0.as_deref(),
        patched_flag(&stored.permission, body.permission.as_deref()).0.as_deref(),
    )
    .await?;
    let cwd = match body.cwd.as_deref() {
        Some(cwd) => Some(validated_cwd(paths, cwd).await?),
        None => None,
    };

    let updated = update(paths, |tasks| {
        let task = tasks.iter_mut().find(|task| task.id == id)?;
        if let Some(name) = body.name.as_deref() {
            task.name = sanitize_name(Some(name), &task.name);
        }
        // Both of these move the next occurrence, and both must move it from
        // *now* rather than from whatever was stored: a task re-enabled at noon
        // whose stored time was this morning would otherwise fire the instant
        // the runner next looked, which is precisely the catch-up this feature
        // does not do.
        let rescheduled = expr.is_some() || body.enabled.0.is_some_and(|next| next != task.enabled);
        task.source = source;
        if let Some(expr) = expr {
            task.expr = expr;
        }
        if let Some(cwd) = cwd {
            task.cwd = cwd;
        }
        if let Some(prompt) = body.prompt.0 {
            task.prompt = prompt;
        }
        if let Some(enabled) = body.enabled.0 {
            task.enabled = enabled;
        }
        task.model = patched_flag(&task.model, body.model.as_deref());
        task.effort = patched_flag(&task.effort, body.effort.as_deref());
        task.permission = patched_flag(&task.permission, body.permission.as_deref());
        if let Some(skip) = body.skip_if_busy.0 {
            task.skip_if_busy = skip;
        }
        if rescheduled {
            task.next_run_at = Nullable(
                task.enabled
                    .then(|| cron::next_occurrence(&task.expr, SystemTime::now()))
                    .flatten()
                    .map(iso8601),
            );
        }
        task.updated_at = now_iso();
        Some(task.clone())
    })
    .await
    .map_err(write_failed)?;

    Ok(Json(updated.ok_or_else(|| ApiError::not_found("scheduled task not found"))?))
}

/// Delete a scheduled task. Sessions it already started are left alone.
#[api(id = "delete_scheduled_task", group = "schedule", summary = "Delete a scheduled task")]
pub async fn delete_scheduled_task(State(state): State<AppState>, ApiPath(id): ApiPath<String>) -> Result<Json<SendResult>, ApiError> {
    let removed = update(state.paths.as_ref(), |tasks| {
        let before = tasks.len();
        tasks.retain(|task| task.id != id);
        (tasks.len() != before).then_some(())
    })
    .await
    .map_err(write_failed)?;

    removed.ok_or_else(|| ApiError::not_found("scheduled task not found"))?;
    Ok(Json(SendResult { accepted: true }))
}

/// Register this subsystem's routes.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router
        .get("/api/scheduled-tasks", list_scheduled_tasks)
        .post("/api/scheduled-tasks", create_scheduled_task)
        .patch("/api/scheduled-tasks/{id}", update_scheduled_task)
        .delete("/api/scheduled-tasks/{id}", delete_scheduled_task)
}

/// Everything gotcha cannot express about those routes.
///
/// `POST` re-states its `200` through [`ExtraResponses::body`] rather than
/// [`ExtraResponses::describe`], because that is the only way to drop the
/// `default` response gotcha derives from the handler's `Result`; the other two
/// fallible routes lose theirs to their documented `404`.
///
/// The `400`s are documented rather than left to `default` for a reason that is
/// this feature's own. The flag checks above answer with the CLI's actual effort
/// ladder — *"unknown claude effort: max (expected one of low, medium, high)"* —
/// and that sentence is the entire payoff of checking at save time instead of at
/// three in the morning. An undocumented status is typed away by
/// `openapi-typescript`, so the form would be left with nothing to show but its
/// own guess at what went wrong.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe("GET", "/api/scheduled-tasks", 200, "Every scheduled task this instance runs")
        .body::<ScheduledTask>("POST", "/api/scheduled-tasks", 200, "Scheduled task created")
        .error(
            "POST",
            "/api/scheduled-tasks",
            400,
            "Unusable schedule, working directory, prompt, or a flag value this CLI does not take",
        )
        .error(
            "POST",
            "/api/scheduled-tasks",
            409,
            "This instance already holds the maximum number of scheduled tasks",
        )
        .describe("PATCH", "/api/scheduled-tasks/{id}", 200, "Scheduled task updated")
        .error(
            "PATCH",
            "/api/scheduled-tasks/{id}",
            400,
            "Unusable schedule, working directory, prompt, or a flag value this CLI does not take",
        )
        .error("PATCH", "/api/scheduled-tasks/{id}", 404, "Scheduled task not found")
        .describe("DELETE", "/api/scheduled-tasks/{id}", 200, "Scheduled task deleted")
        .error("DELETE", "/api/scheduled-tasks/{id}", 404, "Scheduled task not found");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(dir.path().to_path_buf(), "default");
        (dir, paths)
    }

    fn task(id: &str) -> ScheduledTask {
        ScheduledTask {
            id: id.to_string(),
            name: "nightly".into(),
            enabled: true,
            source: Source::Claude,
            expr: "0 9 * * *".into(),
            cwd: "/tmp".into(),
            prompt: "review the diff".into(),
            model: Nullable(None),
            effort: Nullable(None),
            permission: Nullable(None),
            skip_if_busy: true,
            created_at: now_iso(),
            updated_at: now_iso(),
            next_run_at: Nullable(None),
            last_run_at: Nullable(None),
            last_status: Nullable(None),
            last_error: Nullable(None),
            last_session_id: Nullable(None),
        }
    }

    /// Put raw payloads in the table, as a build that wrote them did.
    ///
    /// The payload column is opaque to the schema, so this is the level a
    /// damaged or older row has to be staged at to exercise the reader's
    /// per-field tolerance.
    async fn seed_payloads(paths: &Paths, payloads: &[&str]) {
        let db = store(paths);
        db.write(|tx| {
            for (index, payload) in payloads.iter().enumerate() {
                tx.execute(
                    "INSERT INTO scheduled_tasks (id, payload) VALUES (?1, ?2)",
                    (format!("seed-{index}"), *payload),
                )?;
            }
            Ok(())
        })
        .expect("seed the task table");
    }

    #[tokio::test]
    async fn the_list_round_trips() {
        let (_dir, paths) = paths();
        assert!(read_tasks(&paths).await.is_empty(), "an empty database is no tasks");

        write_tasks(&paths, &[task("a"), task("b")]).await.expect("write");
        let read = read_tasks(&paths).await;
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].id, "a");
        assert_eq!(read[1].expr, "0 9 * * *");
    }

    /// What a restart is, from the store's point of view.
    #[tokio::test]
    async fn a_task_survives_the_database_being_reopened() {
        let (_dir, paths) = paths();
        write_tasks(&paths, &[task("a")]).await.expect("write");

        let reopened = Db::open(paths.instance_db());
        let stored: String = reopened
            .read(|conn| conn.query_row("SELECT id FROM scheduled_tasks", [], |row| row.get(0)))
            .expect("the row is there for anyone who opens the file");
        assert_eq!(stored, "a");
    }

    #[tokio::test]
    async fn a_hand_edited_entry_costs_only_the_field_it_broke() {
        let (_dir, paths) = paths();
        seed_payloads(
            &paths,
            &[r#"{"id":"a","expr":"0 9 * * *","cwd":7,"enabled":"yes","lastStatus":"nonsense"}"#],
        )
        .await;

        let tasks = read_tasks(&paths).await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].expr, "0 9 * * *");
        assert_eq!(tasks[0].cwd, "", "a number where a path belongs is no path");
        assert!(!tasks[0].enabled, "an unreadable `enabled` never starts a CLI");
        assert_eq!(tasks[0].last_status, Nullable(None));
        assert_eq!(tasks[0].name, "task 1", "a missing name is numbered by position");
        assert!(tasks[0].skip_if_busy, "the safe default survives a missing field");
    }

    /// The elision the runner's five-second tick takes: an idle tick must not
    /// rewrite every row with the bytes it already had, seventeen thousand times
    /// a day, in seventeen thousand transactions.
    #[tokio::test]
    async fn an_update_that_changes_nothing_does_not_write() {
        let (_dir, paths) = paths();
        write_tasks(&paths, &[task("a")]).await.expect("write");
        let db = crate::db::at(paths.instance_db());
        let changes_before: i64 = db
            .read(|conn| conn.query_row("SELECT total_changes()", [], |row| row.get(0)))
            .expect("total_changes");

        let outcome = update(&paths, |_| None::<()>).await.expect("update");
        assert!(outcome.is_none());

        let changes_after: i64 = db
            .read(|conn| conn.query_row("SELECT total_changes()", [], |row| row.get(0)))
            .expect("total_changes");
        assert_eq!(changes_before, changes_after, "the runner's idle tick must not touch a row");
    }

    #[tokio::test]
    async fn an_update_finds_its_task_or_says_so() {
        let (_dir, paths) = paths();
        write_tasks(&paths, &[task("a")]).await.expect("write");

        let found = update(&paths, |tasks| {
            let task = tasks.iter_mut().find(|task| task.id == "a")?;
            task.enabled = false;
            Some(task.clone())
        })
        .await
        .expect("update");
        assert!(!found.expect("found").enabled);
        assert!(!read_tasks(&paths).await[0].enabled);

        let missing = update(&paths, |tasks| tasks.iter_mut().find(|task| task.id == "zzz").cloned())
            .await
            .expect("update");
        assert!(missing.is_none());
    }

    /// A save writes the database and nothing else.
    ///
    /// The two halves belong together. Nothing creates `scheduled-tasks.json`
    /// any more, so there is no longer a plaintext copy of the user's prompts
    /// and source paths beside the store; and the store that absorbed them is
    /// itself unreadable to anyone else, which is where this module's `0600`
    /// rule now lives.
    #[tokio::test]
    #[cfg(unix)]
    async fn a_save_writes_no_file_and_leaves_the_store_unreadable() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, paths) = paths();
        // Pin the umask so this asserts on what the code did rather than on
        // whatever the machine running the tests happens to default to.
        // SAFETY: `umask` is always safe to call; the previous value is restored.
        let previous = unsafe { libc::umask(0o022) };
        let result = write_tasks(&paths, &[task("a")]).await;
        // SAFETY: as above.
        unsafe { libc::umask(previous) };
        result.expect("write");

        assert!(!paths.scheduled_tasks_file().exists(), "no task list is written out beside the database");
        assert!(!paths.scheduled_tasks_file().with_extension("json.tmp").exists());
        let mode = std::fs::metadata(paths.instance_db()).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn a_blank_patch_clears_a_flag_and_an_absent_one_leaves_it() {
        let current = Nullable(Some("opus".to_string()));
        assert_eq!(patched_flag(&current, None), current, "absent leaves it alone");
        assert_eq!(patched_flag(&current, Some("   ")), Nullable(None), "blank clears it");
        assert_eq!(patched_flag(&current, Some(" sonnet ")), Nullable(Some("sonnet".into())));
    }

    #[test]
    fn a_blank_flag_means_the_same_thing_on_both_routes() {
        // Create hands `flag` straight to the validator; update goes through
        // `patched_flag` first. Both have to fold blank to "no value", or
        // `{"model": ""}` is a 400 on one route and a clear on the other.
        assert_eq!(flag(Some("")), None);
        assert_eq!(flag(Some("   ")), None);
        assert_eq!(flag(None), None);
        assert_eq!(flag(Some(" opus ")), Some("opus".to_string()));
    }

    #[test]
    fn a_name_is_trimmed_capped_and_defaulted() {
        assert_eq!(sanitize_name(Some("  nightly  "), "task 1"), "nightly");
        assert_eq!(sanitize_name(Some("   "), "task 1"), "task 1");
        assert_eq!(sanitize_name(None, "task 3"), "task 3");
        assert_eq!(sanitize_name(Some(&"x".repeat(MAX_NAME + 40)), "task 1").chars().count(), MAX_NAME);
    }

    #[tokio::test]
    async fn each_cli_is_checked_against_its_own_permission_modes() {
        let (_dir, paths) = paths();
        // The values the composer offers for each, and neither CLI's are the
        // other's: `acceptEdits` is not a sandbox and `read-only` is not a
        // permission mode.
        assert!(validated_flags(&paths, Source::Claude, None, None, Some("acceptEdits")).await.is_ok());
        assert!(validated_flags(&paths, Source::Codex, None, None, Some("read-only")).await.is_ok());
        assert!(validated_flags(&paths, Source::Claude, None, None, Some("read-only")).await.is_err());
        assert!(validated_flags(&paths, Source::Codex, None, None, Some("acceptEdits")).await.is_err());
        assert!(validated_flags(&paths, Source::Claude, None, None, Some("nonsense")).await.is_err());
    }

    #[tokio::test]
    async fn a_claude_effort_is_checked_against_the_ladder_the_model_actually_has() {
        let (_dir, paths) = paths();
        assert!(validated_flags(&paths, Source::Claude, Some("opus"), Some("max"), None).await.is_ok());
        assert!(
            validated_flags(&paths, Source::Claude, Some("haiku"), Some("high"), None).await.is_err(),
            "the haiku tier takes no --effort at all"
        );
        assert!(
            validated_flags(&paths, Source::Claude, Some("claude-opus-4-5"), Some("xhigh"), None)
                .await
                .is_err(),
            "that generation predates the rung"
        );
        assert!(validated_flags(&paths, Source::Claude, None, Some("nonsense"), None).await.is_err());
    }

    #[tokio::test]
    async fn a_codex_effort_is_let_through_when_this_machine_has_never_run_codex() {
        let (_dir, paths) = paths();
        // No `models_cache.json`, so nothing here is entitled to an opinion —
        // refusing would hide a rung a new model was released for.
        assert!(validated_flags(&paths, Source::Codex, None, Some("xhigh"), None).await.is_ok());

        // With a cache, the rungs it names are the ones that count.
        std::fs::create_dir_all(paths.codex_dir()).expect("codex dir");
        std::fs::write(
            paths.codex_models_cache(),
            r#"{"models":[{"slug":"gpt-5.5","visibility":"list","supported_reasoning_levels":[{"effort":"low"},{"effort":"high"}]}]}"#,
        )
        .expect("seed");
        assert!(validated_flags(&paths, Source::Codex, None, Some("high"), None).await.is_ok());
        assert!(validated_flags(&paths, Source::Codex, None, Some("xhigh"), None).await.is_err());
        assert!(
            validated_flags(&paths, Source::Codex, Some("gpt-5.5"), Some("low"), None).await.is_ok(),
            "a named model answers for itself"
        );
    }

    #[tokio::test]
    async fn a_payload_written_before_sources_existed_reads_as_claude() {
        let (_dir, paths) = paths();
        seed_payloads(
            &paths,
            &[
                r#"{"id":"a","expr":"0 9 * * *"}"#,
                r#"{"id":"b","expr":"0 9 * * *","source":"codex"}"#,
                r#"{"id":"c","expr":"0 9 * * *","source":"gopher"}"#,
            ],
        )
        .await;

        let tasks = read_tasks(&paths).await;
        assert_eq!(tasks[0].source, Source::Claude);
        assert_eq!(tasks[1].source, Source::Codex);
        assert_eq!(tasks[2].source, Source::Claude, "an unknown CLI is not a third one");
    }

    #[test]
    fn a_stored_stamp_round_trips_through_the_parser() {
        let now = SystemTime::now();
        let parsed = parse_iso(&iso8601(now)).expect("parse");
        let drift = parsed.duration_since(now).or_else(|_| now.duration_since(parsed)).expect("either way");
        assert!(drift.as_millis() <= 1, "iso8601 keeps milliseconds, so the round trip is exact");
        assert_eq!(parse_iso("yesterday"), None);
    }
}
