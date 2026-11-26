//! The `tck` entry: no CLI, only the states the A2A TCK asks for
//! (`docs/a2a.md` §5; the TCK's `scenarios/*.feature`).
//!
//! The TCK says what it wants by the message id: `tck-complete-task-…`,
//! `tck-input-required-…`, `test-resubscribe-message-id…`. Only an instance
//! started with `DROPPEN_A2A_TCK=1` can have such an entry, and nothing else
//! comes here. Everything else is the server's own: tokens, scoping, the
//! store, the streams, the errors. One difference: the TCK sends the same
//! message id many times in one run, so a `tck` entry does not hold a
//! `(caller, messageId)` to one task.

use std::sync::Arc;

use serde_json::json;

use crate::engine::{context_not_found, A2a, Caller, Created, Making, Sent};
use crate::error::A2aError;
use crate::wire::{Artifact, Message, Part, PartContent, Role, SendMessageConfiguration, TaskState};

/// The scenario `message_id` names, played on `task_id`.
async fn play(a2a: Arc<A2a>, task_id: String, message_id: String) {
    let is = |prefix: &str| message_id.starts_with(prefix);
    let file = || Part {
        content: PartContent::Raw(b"TCK file content".to_vec()),
        metadata: None,
        filename: "output.txt".into(),
        media_type: "text/plain".into(),
    };
    let artifact = |name: &str, parts: Vec<Part>| Artifact {
        artifact_id: uuid::Uuid::new_v4().to_string(),
        name: name.into(),
        parts,
        ..Artifact::default()
    };
    let done = |a2a: &A2a| a2a.set_state(&task_id, TaskState::Completed, None);
    let streamed_text = [
        ("tck-stream-001", "Stream hello from TCK"),
        ("tck-stream-003", "Stream task lifecycle"),
        ("tck-stream-ordering-001", "Ordered output"),
        ("tck-stream-artifact-text", "Streamed text content"),
    ];
    if is("tck-artifact-text") {
        a2a.add_artifact(&task_id, artifact("text", vec![Part::text("Generated text content")]), false, true);
        done(&a2a);
    } else if is("tck-artifact-file-url") {
        let url = Part {
            content: PartContent::Url("https://example.com/output.txt".into()),
            metadata: None,
            filename: "output.txt".into(),
            media_type: "text/plain".into(),
        };
        a2a.add_artifact(&task_id, artifact("file", vec![url]), false, true);
        done(&a2a);
    } else if is("tck-artifact-file") {
        a2a.add_artifact(&task_id, artifact("file", vec![file()]), false, true);
        done(&a2a);
    } else if is("tck-artifact-data") {
        a2a.add_artifact(
            &task_id,
            artifact("data", vec![Part::data(json!({"key": "value", "count": 42}))]),
            false,
            true,
        );
        done(&a2a);
    } else if is("tck-input-required") {
        a2a.set_state(&task_id, TaskState::InputRequired, Some("Input required"));
    } else if is("tck-reject-task") {
        a2a.set_state(&task_id, TaskState::Rejected, Some("rejected"));
    } else if is("tck-stream-002") {
        done(&a2a);
    } else if let Some((_, text)) = streamed_text.iter().find(|(prefix, _)| is(prefix)) {
        a2a.set_state(&task_id, TaskState::Working, None);
        a2a.add_artifact(&task_id, artifact("stream", vec![Part::text(*text)]), false, true);
        done(&a2a);
    } else if is("tck-stream-artifact-file") {
        a2a.set_state(&task_id, TaskState::Working, None);
        a2a.add_artifact(&task_id, artifact("file", vec![file()]), false, true);
        done(&a2a);
    } else if is("tck-stream-artifact-chunked") {
        a2a.set_state(&task_id, TaskState::Working, None);
        let id = uuid::Uuid::new_v4().to_string();
        let chunk = |text: &str| Artifact {
            artifact_id: id.clone(),
            name: "chunked".into(),
            parts: vec![Part::text(text)],
            ..Artifact::default()
        };
        a2a.add_artifact(&task_id, chunk("chunk-1 "), false, false);
        a2a.add_artifact(&task_id, chunk("chunk-2"), true, true);
        done(&a2a);
    } else if is("test-resubscribe-message-id") {
        a2a.set_state(&task_id, TaskState::Working, None);
        tokio::time::sleep(a2a.settings().tck_streaming_timeout * 2).await;
        done(&a2a);
    } else {
        // `tck-complete-task`, and every other id.
        a2a.set_state(&task_id, TaskState::Completed, Some("Hello from TCK"));
    }
    a2a.scenarios_done(&task_id);
}

impl A2a {
    /// Run the scenario of `message_id` on `task_id`, cancellable.
    fn start_scenario(self: &Arc<Self>, task_id: &str, message_id: &str) {
        let handle = tokio::spawn(play(self.clone(), task_id.to_string(), message_id.to_string()));
        self.scenarios_insert(task_id, handle.abort_handle());
    }
}

/// `SendMessage` and `SendStreamingMessage` on a `tck` entry.
pub(crate) async fn send(a2a: &Arc<A2a>, who: &Caller, message: Message, config: SendMessageConfiguration, blocking: bool) -> Result<Sent, A2aError> {
    if message.message_id.starts_with("tck-message-response") {
        let context_id = if message.context_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            message.context_id.clone()
        };
        return Ok(Sent::Message(Message {
            message_id: uuid::Uuid::new_v4().to_string(),
            context_id,
            role: Role::Agent,
            parts: vec![Part::text("Direct message response")],
            ..Message::default()
        }));
    }
    let history_length = config.history_length;
    if !message.task_id.is_empty() {
        let task = a2a.scoped_task(who, &message.task_id)?;
        if !message.context_id.is_empty() && message.context_id != task.context_id {
            return Err(A2aError::invalid("contextId is not the task's context"));
        }
        if task.state.is_terminal() {
            return Err(A2aError::unsupported(
                "the task is finished: send a new message with its contextId and no taskId to continue the session",
            ));
        }
        if task.state == TaskState::InputRequired && !who.entry.allow_remote_answers {
            return Err(A2aError::unsupported("this agent takes no answers over A2A").with_code(crate::backend::ErrorCode::RemoteAnswersDisabled));
        }
        let waiting = task.state == TaskState::InputRequired;
        a2a.note_message(&task.id, message.clone());
        if waiting {
            a2a.set_state(&task.id, TaskState::Working, None);
        }
        let follow = a2a.follow(Some(who), &task.id, history_length).ok_or_else(A2aError::task_not_found)?;
        if waiting {
            a2a.start_scenario(&task.id, &message.message_id);
        }
        return Ok(Sent::Task {
            follow: Box::new(follow),
            blocking,
        });
    }
    let context = if message.context_id.is_empty() {
        None
    } else {
        Some(
            a2a.store()
                .context_for(&who.entry.id, &who.caller, &message.context_id)
                .ok_or_else(context_not_found)?,
        )
    };
    let message_id = message.message_id.clone();
    let Created::New(follow) = a2a.create(who, message, context, Making::Tck, config.task_push_notification_config)? else {
        return Err(A2aError::internal("a tck task is never a duplicate"));
    };
    a2a.start_scenario(follow.task_id(), &message_id);
    Ok(Sent::Task {
        follow: follow.history(history_length),
        blocking,
    })
}
