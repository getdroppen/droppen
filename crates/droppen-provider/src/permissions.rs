//! Interactive tool permission prompts, answered from any connected device.
//!
//! Mirrors [`crate::questions`] for tool approvals (ACP `session/request_permission`
//! and Claude Code `can_use_tool`): one row per request in `session_permissions`,
//! compare-and-set out of `pending` before writing back to the agent so two
//! devices racing for one prompt cannot both decide it.

use std::collections::HashSet;
use std::sync::Arc;

use rusqlite::OptionalExtension;
use serde_json::Value;

use droppen_http::openapi_ext::Nullable;
use droppen_instance::db::Db;
use droppen_sessions::delivery::{PermissionOption, PermissionOptionKind, PermissionState, SessionPermission};

use crate::questions::PermissionRequest;

/// How long a settled permission row is kept.
pub const KEEP_FOR_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Most permission rows carried in one session's state.
pub const RECENT_CAP: usize = 20;
/// How often the prune runs, at most.
const PRUNE_EVERY_MS: i64 = 10 * 60 * 1000;
/// Maximum preview length stored for a permission prompt's tool input.
const MAX_INPUT_PREVIEW: usize = 4096;
/// Maximum length stored for a permission prompt's title or reason.
const MAX_TITLE_CHARS: usize = 256;

/// Option id for allowing a single Claude Code tool call.
pub const CLAUDE_ALLOW: &str = "allow";
/// Option id for allowing a Claude Code tool call and applying its suggested rules.
pub const CLAUDE_ALLOW_ALWAYS: &str = "allow_always";
/// Option id for denying a Claude Code tool call.
pub const CLAUDE_DENY: &str = "deny";

/// What an agent asked the user to approve.
#[derive(Debug, Clone)]
pub struct PermissionSpec {
    /// Globally unique id identifying this prompt across devices.
    pub request_id: String,
    /// Session that requested permission.
    pub session_id: String,
    /// Tool call id the request refers to, when provided by the agent.
    pub tool_call_id: Option<String>,
    /// Short human-readable summary of the requested action.
    pub title: String,
    /// Tool name or kind (`Bash`, `Write`, `shell`, `edit`, …).
    pub tool_name: Option<String>,
    /// Optional reason why permission is requested.
    pub reason: Option<String>,
    /// Redacted preview of the tool input.
    pub input: Option<String>,
    /// Options offered by the agent.
    pub options: Vec<PermissionOption>,
}

/// One row of `session_permissions`.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    /// Globally unique id identifying this prompt.
    pub request_id: String,
    /// Session that requested permission.
    pub session_id: String,
    /// Tool call id the request refers to, when provided.
    pub tool_call_id: Option<String>,
    /// Short human-readable summary of the requested action.
    pub title: String,
    /// Tool name or kind.
    pub tool_name: Option<String>,
    /// Optional reason why permission is requested.
    pub reason: Option<String>,
    /// Redacted preview of the tool input.
    pub input: Option<String>,
    /// Options offered by the agent.
    pub options: Vec<PermissionOption>,
    /// Lifecycle state of the prompt.
    pub state: PermissionState,
    /// Option id selected by the user, if resolved.
    pub selected_option_id: Option<String>,
    /// The protocol response written for this decision (used by Claude Code
    /// residents to re-send a settled response after a server restart).
    pub delivery: Option<Value>,
    /// User who answered, when authentication is enabled.
    pub answered_by: Option<String>,
    /// Decision timestamp in epoch milliseconds.
    pub answered_ms: Option<i64>,
    /// Creation timestamp in epoch milliseconds.
    pub created_ms: i64,
}

impl Stored {
    /// The row in the shape the API speaks.
    pub fn wire(&self) -> SessionPermission {
        SessionPermission {
            request_id: self.request_id.clone(),
            tool_call_id: Nullable(self.tool_call_id.clone()),
            title: self.title.clone(),
            tool_name: Nullable(self.tool_name.clone()),
            reason: Nullable(self.reason.clone()),
            input: Nullable(self.input.clone()),
            options: self.options.clone(),
            state: self.state,
            selected_option_id: Nullable(self.selected_option_id.clone()),
            answered_by: Nullable(self.answered_by.clone()),
            answered_at: Nullable(self.answered_ms.map(droppen_accounts::rest::iso_ms)),
            created_at: droppen_accounts::rest::iso_ms(self.created_ms),
        }
    }
}

/// The permission store, over the instance database.
#[derive(Debug)]
pub struct Permissions {
    db: Arc<Db>,
    prune_gate: droppen_instance::db::PruneGate,
}

const COLUMNS: &str = "request_id, session_id, tool_call_id, title, tool_name, reason, input, options, state, selected_option_id, delivery, answered_by, answered_ms, created_ms";

fn stored_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Stored> {
    let options_raw: String = row.get(7)?;
    let delivery_raw: Option<String> = row.get(10)?;
    Ok(Stored {
        request_id: row.get(0)?,
        session_id: row.get(1)?,
        tool_call_id: row.get(2)?,
        title: row.get(3)?,
        tool_name: row.get(4)?,
        reason: row.get(5)?,
        input: row.get(6)?,
        options: serde_json::from_str(&options_raw).unwrap_or_default(),
        state: PermissionState::from_stored(&row.get::<_, String>(8)?),
        selected_option_id: row.get(9)?,
        delivery: delivery_raw.and_then(|text| serde_json::from_str(&text).ok()),
        answered_by: row.get(11)?,
        answered_ms: row.get(12)?,
        created_ms: row.get(13)?,
    })
}

impl Permissions {
    /// The store over an open instance database.
    pub fn new(db: Arc<Db>) -> Self {
        Self {
            db,
            prune_gate: droppen_instance::db::PruneGate::new(PRUNE_EVERY_MS),
        }
    }

    /// Write down a pending permission prompt. `true` when it is new.
    pub fn open(&self, spec: &PermissionSpec, now: i64) -> bool {
        self.prune_now_and_then(now);
        let options = serde_json::to_string(&spec.options).unwrap_or_else(|_| "[]".to_string());
        let inserted = self.db.write(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO session_permissions
                     (request_id, session_id, tool_call_id, title, tool_name, reason, input, options, state, created_ms, updated_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?9)",
                rusqlite::params![
                    spec.request_id,
                    spec.session_id,
                    spec.tool_call_id,
                    spec.title,
                    spec.tool_name,
                    spec.reason,
                    spec.input,
                    options,
                    now
                ],
            )
        });
        match inserted {
            Ok(changed) => changed > 0,
            Err(error) => {
                tracing::warn!(path = %self.db.path().display(), request_id = %spec.request_id, %error, "cannot record a permission prompt");
                false
            }
        }
    }

    /// One permission prompt, by request id.
    pub fn get(&self, request_id: &str) -> Option<Stored> {
        self.db
            .read(|conn| {
                conn.query_row(
                    &format!("SELECT {COLUMNS} FROM session_permissions WHERE request_id = ?1"),
                    [request_id],
                    stored_row,
                )
                .optional()
            })
            .ok()
            .flatten()
    }

    /// Compare-and-set a permission prompt out of `pending` into `to`.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve(
        &self,
        request_id: &str,
        to: PermissionState,
        selected_option_id: Option<&str>,
        delivery: Option<&Value>,
        by: Option<&str>,
        now: i64,
    ) -> Result<Stored, Option<Box<Stored>>> {
        let decided = to == PermissionState::Resolved || by.is_some();
        let delivery = delivery.map(Value::to_string);
        let changed = self.db.write(|tx| {
            tx.execute(
                "UPDATE session_permissions
                    SET state = ?2,
                        selected_option_id = COALESCE(?3, selected_option_id),
                        delivery = COALESCE(?4, delivery),
                        answered_by = CASE WHEN ?6 THEN ?5 ELSE answered_by END,
                        answered_ms = CASE WHEN ?6 THEN ?7 ELSE answered_ms END,
                        updated_ms = ?7
                  WHERE request_id = ?1 AND state = 'pending'",
                rusqlite::params![request_id, to.as_str(), selected_option_id, delivery, by, decided, now],
            )
        });
        match changed {
            Ok(1) => self.get(request_id).ok_or(None),
            Ok(_) => Err(self.get(request_id).map(Box::new)),
            Err(error) => {
                tracing::warn!(path = %self.db.path().display(), request_id, %error, "cannot record a permission decision");
                Err(self.get(request_id).map(Box::new))
            }
        }
    }

    /// Close a pending permission prompt without a user choice (turn stopped,
    /// agent withdrew it, or process exited).
    pub fn close(&self, request_id: &str, to: PermissionState, now: i64) -> bool {
        self.resolve(request_id, to, None, None, None, now).is_ok()
    }

    /// Move a row back from `from` to `pending`, forgetting its decision (for a
    /// write failure to the CLI).
    pub fn rewind(&self, request_id: &str, from: PermissionState, now: i64) -> bool {
        let rewound = self.db.write(|tx| {
            tx.execute(
                "UPDATE session_permissions
                    SET state = 'pending', selected_option_id = NULL, delivery = NULL, answered_by = NULL, answered_ms = NULL, updated_ms = ?3
                  WHERE request_id = ?1 AND state = ?2",
                rusqlite::params![request_id, from.as_str(), now],
            )
        });
        matches!(rewound, Ok(1))
    }

    /// Pending permission prompts for one session, oldest first.
    pub fn pending_for(&self, session: &str) -> Vec<Stored> {
        self.db
            .read(|conn| {
                let mut stmt = conn.prepare_cached(&format!(
                    "SELECT {COLUMNS} FROM session_permissions
                      WHERE session_id = ?1 AND state = 'pending' ORDER BY created_ms, request_id"
                ))?;
                let rows = stmt.query_map([session], stored_row)?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default()
    }

    /// Cancel every pending permission prompt whose session has no surviving
    /// process in `held`.
    pub fn cancel_unheld(&self, held: &HashSet<String>, now: i64) -> usize {
        let pending: Vec<(String, String)> = self
            .db
            .read(|conn| {
                let mut stmt = conn.prepare("SELECT request_id, session_id FROM session_permissions WHERE state = 'pending'")?;
                let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default();
        pending
            .into_iter()
            .filter(|(_, session)| !held.contains(session))
            .filter(|(request_id, _)| self.close(request_id, PermissionState::Cancelled, now))
            .count()
    }

    /// Recent permission prompts for one session, oldest first.
    pub fn for_session(&self, session: &str, now: i64) -> Vec<SessionPermission> {
        let read = self.db.read(|conn| {
            let mut stmt = conn.prepare_cached(&format!(
                "SELECT {COLUMNS} FROM session_permissions
                  WHERE session_id = ?1 AND (state = 'pending' OR created_ms >= ?2)
                  ORDER BY created_ms DESC, request_id DESC
                  LIMIT ?3"
            ))?;
            let rows = stmt.query_map(rusqlite::params![session, now - KEEP_FOR_MS, RECENT_CAP as i64], stored_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        });
        match read {
            Ok(rows) => rows.iter().rev().map(Stored::wire).collect(),
            Err(error) => {
                tracing::warn!(path = %self.db.path().display(), %error, "cannot read a session's permissions");
                Vec::new()
            }
        }
    }

    /// Forget settled permission rows older than [`KEEP_FOR_MS`].
    pub fn prune(&self, now: i64) {
        self.db.commit("the permission prune", |tx| {
            tx.execute(
                "DELETE FROM session_permissions WHERE created_ms < ?1 AND state != 'pending'",
                [now - KEEP_FOR_MS],
            )?;
            Ok(())
        });
    }

    fn prune_now_and_then(&self, now: i64) {
        if self.prune_gate.due(now) {
            self.prune(now);
        }
    }
}

/// Epoch milliseconds.
pub use droppen_base::time::now_ms;

/// Redact and clip a permission prompt's input preview.
pub fn format_input_preview(input: &Value, redact: impl Fn(&str) -> String) -> Option<String> {
    if input.is_null() {
        return None;
    }
    let raw = match input {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return None;
    }
    Some(droppen_base::text::clip(&redact(trimmed), MAX_INPUT_PREVIEW))
}

/// Redact and clip a short label (title or reason).
pub fn clip_label(text: &str, redact: impl Fn(&str) -> String) -> String {
    droppen_base::text::clip(&redact(text.trim()), MAX_TITLE_CHARS)
}

/// Options presented for a Claude Code `can_use_tool` permission prompt.
pub fn claude_options(request: &PermissionRequest) -> Vec<PermissionOption> {
    let mut options = vec![PermissionOption {
        id: CLAUDE_ALLOW.to_string(),
        name: "Allow".to_string(),
        kind: PermissionOptionKind::AllowOnce,
    }];
    if request.permission_suggestions.as_ref().and_then(Value::as_array).is_some_and(|list| !list.is_empty()) {
        options.push(PermissionOption {
            id: CLAUDE_ALLOW_ALWAYS.to_string(),
            name: "Always allow".to_string(),
            kind: PermissionOptionKind::AllowAlways,
        });
    }
    options.push(PermissionOption {
        id: CLAUDE_DENY.to_string(),
        name: "Deny".to_string(),
        kind: PermissionOptionKind::RejectOnce,
    });
    options
}

/// Short human-readable title for a Claude Code `can_use_tool` prompt.
pub fn claude_title(request: &PermissionRequest, redact: impl Fn(&str) -> String) -> String {
    let tool = if request.tool_name.is_empty() {
        "Tool"
    } else {
        request.tool_name.as_str()
    };
    let detail = request
        .input
        .get("command")
        .or_else(|| request.input.get("file_path"))
        .or_else(|| request.input.get("path"))
        .or_else(|| request.input.get("url"))
        .or_else(|| request.input.get("pattern"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .and_then(|text| text.lines().next());
    let title = match detail {
        Some(detail) => format!("{tool}: {detail}"),
        None => tool.to_string(),
    };
    clip_label(&title, redact)
}

/// Claude Code `can_use_tool` allow response, echoing `updatedInput` verbatim
/// and attaching `updatedPermissions` when the user chose "Always allow".
pub fn claude_allow(input: &Value, updated_permissions: Option<&Value>) -> Value {
    let mut response = serde_json::Map::new();
    response.insert("behavior".to_string(), Value::String("allow".to_string()));
    response.insert("updatedInput".to_string(), input.clone());
    if let Some(permissions) = updated_permissions {
        response.insert("updatedPermissions".to_string(), permissions.clone());
    }
    Value::Object(response)
}

/// Claude Code `can_use_tool` deny response when the user rejects or cancels.
pub fn claude_deny(tool_name: &str, cancelled: bool) -> Value {
    let tool = if tool_name.is_empty() { "this tool" } else { tool_name };
    let message = if cancelled {
        format!("Permission request for {tool} was cancelled.")
    } else {
        format!("The user denied permission to use {tool}.")
    };
    crate::questions::deny(&message)
}

/// One-line notification headline for a permission prompt.
pub fn headline(title: &str) -> String {
    if title.trim().is_empty() {
        "Permission requested".to_string()
    } else {
        format!("Permission: {title}")
    }
}
