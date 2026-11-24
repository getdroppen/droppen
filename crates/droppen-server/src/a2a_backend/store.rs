//! What the backend writes down of the tasks it follows, so that a restart
//! can pick them up again ([`super::A2aSessions`]'s `resume`).
//!
//! The protocol side keeps the tasks themselves; what it does not know is
//! which of a session's messages are a task's — the queue ids its messages
//! travel under — and where the task's turn stood. Rows in the instance
//! database's `kv` table, under [`scope::A2A_SESSIONS`]: a `task:<id>` row
//! while a task is followed, gone once it ended or was cancelled, and a
//! `context:<id>` row for each context, kept with the worktree it describes.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use droppen_instance::db::{scope, Db};

/// One message of a task, as it is found again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Message {
    /// The queue id it travels under, which its receipt is kept by.
    pub qid: String,
    /// The `clientMessageId` it was claimed under.
    pub client_message_id: String,
    /// What finds it in the transcript ([`super::turn::probe`]).
    pub probe: String,
}

/// A task the backend follows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskRecord {
    /// Its context.
    pub context_id: String,
    /// Its session's CLI, by wire name.
    pub source: String,
    /// Its session, once known.
    pub session_id: Option<String>,
    /// Where the session runs.
    pub cwd: String,
    /// The worktree's branch, when there is one.
    pub branch: Option<String>,
    /// Who sent it, as the answers it gives are signed.
    pub caller: String,
    /// The message that is its own turn.
    pub main: Message,
    /// The messages added to it while it ran.
    #[serde(default)]
    pub appends: Vec<Message>,
    /// Whether it was reported as taken into its turn.
    #[serde(default)]
    pub turn_started: bool,
    /// The question it was reported as waiting on, by request id.
    #[serde(default)]
    pub asked: Option<String>,
}

/// A context: what its session was started with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextRecord {
    /// Where its session runs.
    pub cwd: String,
    /// The worktree's branch, when there is one.
    pub branch: Option<String>,
    /// The commit the worktree's branch started from.
    pub base: Option<String>,
    /// The entry's model when the session started, which every later message
    /// asks for too, so a new process for it runs as the first did.
    pub model: Option<String>,
    /// The entry's effort, likewise.
    pub effort: Option<String>,
    /// The entry's permission mode, likewise.
    pub permission: Option<String>,
}

/// The rows, over the instance database.
pub(crate) struct Store(Arc<Db>);

impl Store {
    /// The store in `db`.
    pub(crate) fn new(db: Arc<Db>) -> Self {
        Self(db)
    }

    fn get<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Option<T> {
        let raw = self.0.kv_get(scope::A2A_SESSIONS, key)?;
        match serde_json::from_str(&raw) {
            Ok(value) => Some(value),
            Err(error) => {
                tracing::warn!(key, %error, "an A2A backend row does not read; ignoring it");
                None
            }
        }
    }

    fn put(&self, key: &str, value: &impl Serialize) {
        match serde_json::to_string(value) {
            Ok(raw) => self.0.kv_set(scope::A2A_SESSIONS, key, &raw),
            Err(error) => tracing::warn!(key, %error, "cannot write an A2A backend row"),
        }
    }

    /// The task `id`, while it is followed.
    pub(crate) fn task(&self, id: &str) -> Option<TaskRecord> {
        self.get(&format!("task:{id}"))
    }

    /// Write task `id` down as it now stands.
    pub(crate) fn put_task(&self, id: &str, task: &TaskRecord) {
        self.put(&format!("task:{id}"), task);
    }

    /// Stop following task `id`.
    pub(crate) fn forget_task(&self, id: &str) {
        self.0.kv_remove(scope::A2A_SESSIONS, &format!("task:{id}"));
    }

    /// Context `id`, once its session was started here.
    pub(crate) fn context(&self, id: &str) -> Option<ContextRecord> {
        self.get(&format!("context:{id}"))
    }

    /// Write context `id` down.
    pub(crate) fn put_context(&self, id: &str, context: &ContextRecord) {
        self.put(&format!("context:{id}"), context);
    }
}
