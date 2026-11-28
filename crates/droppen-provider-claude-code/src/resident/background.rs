//! Background task tracking for a resident Claude process.

use super::*;

// ------------------------------------------------------------ background work

/// How many background tasks are described on the wire.
///
/// A ceiling on the payload rather than on the truth: `background` still counts
/// every one of them, and a session with more than this many is a bug being
/// reported, not a list worth sending to a phone once a second.
pub const TASKS_ON_THE_WIRE: usize = 100;

/// What one line of claude's stdout said about one background task.
///
/// Both events that announce a task carry these fields, and `task_started` is
/// the richer one: it alone carries the `tool_use_id` — with a `subagent_type`
/// and the whole prompt beside it — where a `background_tasks_changed` snapshot
/// names only the task, its type and its description. Everything past the id is
/// optional because the CLI omits fields freely, and a task with no description
/// is still a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TaskSeen {
    pub(super) id: String,
    pub(super) tool_use_id: Option<String>,
    pub(super) description: Option<String>,
    pub(super) task_type: Option<String>,
}

impl TaskSeen {
    /// Read one task out of a `task_started` line or a snapshot entry.
    pub(super) fn parse(value: &Value) -> Option<Self> {
        Some(Self {
            id: value.get("task_id").and_then(Value::as_str)?.to_string(),
            tool_use_id: value.get("tool_use_id").and_then(Value::as_str).map(str::to_owned),
            description: value.get("description").and_then(Value::as_str).map(str::to_owned),
            task_type: value.get("task_type").and_then(Value::as_str).map(str::to_owned),
        })
    }
}

/// What one `system:task_progress` line said about one running task.
///
/// The CLI publishes one of these for every live task every few seconds — 16,000
/// of them across six recorded sessions — and this server used to drop them all.
/// They carry the only account of what a background task is *doing*: the tool it
/// is in, what it has spent, how long it has run.
///
/// Pointedly **not** carried over: the line's own `description`. It is a live
/// activity string — "Editing src/components/OptionCard.tsx", "Running Check the
/// diff" — that changes with every tool call, while `BackgroundTask::description`
/// is what the task was *asked* for and must not churn. Every one of the 116
/// tasks on record has a different description in every progress line it wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TaskProgress {
    pub(super) id: String,
    pub(super) tool_use_id: Option<String>,
    pub(super) subagent_type: Option<String>,
    pub(super) last_tool_name: Option<String>,
    pub(super) total_tokens: Option<u64>,
    pub(super) tool_uses: Option<u64>,
    pub(super) duration_ms: Option<u64>,
}

impl TaskProgress {
    /// Read one `task_progress` line, tolerating every field but the id.
    pub(super) fn parse(value: &Value) -> Option<Self> {
        let usage = |key: &str| value.get("usage").and_then(|usage| usage.get(key)).and_then(Value::as_u64);
        let string = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(Self {
            id: value.get("task_id").and_then(Value::as_str)?.to_string(),
            tool_use_id: string("tool_use_id"),
            subagent_type: string("subagent_type"),
            last_tool_name: string("last_tool_name"),
            total_tokens: usage("total_tokens"),
            tool_uses: usage("tool_uses"),
            duration_ms: usage("duration_ms"),
        })
    }
}

/// The live background tasks, in the order this driver first heard of them.
///
/// An ordered map over a `Vec`, not a `HashMap`: there are a handful of these at
/// most — four in the busiest real session on record — so the lookups are
/// cheaper than hashing, and insertion order is the one thing a set could not
/// keep. Order matters because it is what a reader sees: the oldest task is the
/// one that has been running longest, and that is the one worth showing first.
#[derive(Debug, Clone, Default)]
pub(super) struct BackgroundTasks {
    /// `(id, tool_use_id, description, task_type, first_seen)`, oldest first.
    pub(super) entries: Vec<droppen_sessions::BackgroundTask>,
}

impl BackgroundTasks {
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }

    /// Record a task, or fill in what a later line said about a known one.
    ///
    /// Returns whether anything changed. `first_seen` is never rewritten: a
    /// snapshot republished every few seconds must not keep resetting the clock
    /// on work that has been running for half an hour.
    pub(super) fn observe(&mut self, seen: TaskSeen) -> bool {
        if let Some(known) = self.entries.iter_mut().find(|task| task.id == seen.id) {
            let mut changed = false;
            // Only ever filled in, never blanked: the snapshot's shape is
            // thinner than `task_started`'s, and letting it overwrite a
            // description with nothing would lose what the richer line said.
            // The `tool_use_id` is the sharpest case of it — only
            // `task_started` carries one, and every snapshot afterwards omits
            // it, so a rule that took the latest word would erase the join key
            // seconds after learning it.
            if seen.tool_use_id.is_some() && known.tool_use_id.0 != seen.tool_use_id {
                known.tool_use_id = seen.tool_use_id.into();
                changed = true;
            }
            if seen.description.is_some() && known.description.0 != seen.description {
                known.description = seen.description.into();
                changed = true;
            }
            if seen.task_type.is_some() && known.task_type.0 != seen.task_type {
                known.task_type = seen.task_type.into();
                changed = true;
            }
            return changed;
        }
        self.entries.push(droppen_sessions::BackgroundTask {
            id: seen.id,
            tool_use_id: seen.tool_use_id.into(),
            description: seen.description.into(),
            task_type: seen.task_type.into(),
            started_at: Some(iso8601(SystemTime::now())).into(),
            subagent_type: Optional(None),
            last_tool_name: Optional(None),
            total_tokens: Optional(None),
            tool_uses: Optional(None),
            duration_ms: Optional(None),
        });
        true
    }

    /// Fold a `task_progress` line into the task it is about.
    ///
    /// Only ever into a task already known. A progress line names no task type
    /// and carries a description that is not the task's, so a row built from one
    /// would be a worse row — and it would also be a task in `background` that
    /// no snapshot ever named, which is what `awaitingBackground` parks on.
    /// Nothing is lost by the rule: across six recorded sessions and 16,000
    /// progress lines, not one arrived for a task that had not already been
    /// announced.
    ///
    /// Returns whether anything a reader can see changed.
    pub(super) fn progress(&mut self, progress: TaskProgress) -> bool {
        let Some(known) = self.entries.iter_mut().find(|task| task.id == progress.id) else {
            return false;
        };
        let mut changed = false;
        // The same fill-never-blank rule the rest of this type follows, plus the
        // join key: `task_progress` carries a `tool_use_id` where the snapshots
        // that follow a restart carry none, so it can be the line that finally
        // makes a task joinable to the transcript.
        if progress.tool_use_id.is_some() && known.tool_use_id.0 != progress.tool_use_id {
            known.tool_use_id = progress.tool_use_id.into();
            changed = true;
        }
        let mut set = |field: &mut Optional<String>, value: Option<String>| {
            if value.is_some() && field.0 != value {
                *field = Optional(value);
                changed = true;
            }
        };
        set(&mut known.subagent_type, progress.subagent_type);
        set(&mut known.last_tool_name, progress.last_tool_name);
        let mut count = |field: &mut Optional<u64>, value: Option<u64>| {
            if value.is_some() && field.0 != value {
                *field = Optional(value);
                changed = true;
            }
        };
        count(&mut known.total_tokens, progress.total_tokens);
        count(&mut known.tool_uses, progress.tool_uses);
        count(&mut known.duration_ms, progress.duration_ms);
        changed
    }

    /// Drop one task, by id. Returns whether it was there.
    pub(super) fn remove(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|task| task.id != id);
        before != self.entries.len()
    }

    /// Apply a whole-list snapshot: absorb what it names, drop what it does not.
    pub(super) fn snapshot(&mut self, live: Vec<TaskSeen>) -> bool {
        let named: HashSet<&str> = live.iter().map(|task| task.id.as_str()).collect();
        let mut changed = self.entries.iter().any(|task| !named.contains(task.id.as_str()));
        self.entries.retain(|task| named.contains(task.id.as_str()));
        for seen in live {
            changed |= self.observe(seen);
        }
        changed
    }

    /// The first [`TASKS_ON_THE_WIRE`] of them, for the mirror.
    pub(super) fn wire(&self) -> Vec<droppen_sessions::BackgroundTask> {
        self.entries.iter().take(TASKS_ON_THE_WIRE).cloned().collect()
    }
}
