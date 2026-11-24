//! The A2A tables of the instance database (`docs/a2a.md` §4; the schema is
//! steps 12 and 13 of [`droppen_instance::db::SCHEMA`]).
//!
//! Synchronous, like every store over [`Db`]: each call is one indexed query
//! or one transaction over a single connection. A task and the events its
//! change makes are written in one transaction, so a reader of the events
//! never sees one the task row does not account for.

use std::sync::Arc;

use rusqlite::{params, OptionalExtension, Row};

use droppen_instance::db::{Db, PruneGate};

use crate::backend::{EntrySource, SessionRef};
use crate::entry::A2aEntry;
use crate::wire::{Artifact, Message, TaskState};

/// How long the events of a finished task are kept: long enough for a push
/// retry and a late subscriber, short enough that the table follows the open
/// tasks rather than every task ever.
const EVENTS_KEPT_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// The pruning of finished tasks' events runs at most once an hour.
static PRUNE: PruneGate = PruneGate::new(60 * 60 * 1000);

/// A token, as kept: the digest of its secret, never the secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRow {
    /// The id in `dpa_<id>_<secret>`.
    pub id: String,
    /// Its name, for a person; the caller is `a2a:token:<id>`.
    pub name: String,
    /// The entries it reaches.
    pub entries: Vec<String>,
    /// `sha256(secret)`, hex.
    pub secret_hash: String,
    /// When it was made, epoch ms.
    pub created_ms: i64,
    /// When it last proved a request, epoch ms (to the minute).
    pub last_used_ms: Option<i64>,
    /// When it was revoked, epoch ms.
    pub revoked_ms: Option<i64>,
}

/// A context: one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextRow {
    /// The context id.
    pub context_id: String,
    /// The entry it came in through.
    pub entry: String,
    /// Who opened it.
    pub caller: String,
    /// Its session.
    pub session: SessionRef,
    /// When it was opened, epoch ms.
    pub created_ms: i64,
}

/// A task, as kept.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskRow {
    /// The task id.
    pub id: String,
    /// Its context.
    pub context_id: String,
    /// The entry it came in through.
    pub entry: String,
    /// Who sent it.
    pub caller: String,
    /// Its state.
    pub state: TaskState,
    /// The message of its status.
    pub status_message: Option<Message>,
    /// When its status last changed, epoch ms.
    pub status_ms: i64,
    /// Its artifacts.
    pub artifacts: Vec<Artifact>,
    /// The messages it was sent, oldest first.
    pub history: Vec<Message>,
    /// `a2a:<caller>:<messageId>` of its first message.
    pub client_message_id: String,
    /// Whether its first message has gone to the backend.
    pub dispatched: bool,
    /// The number of its newest event.
    pub seq: i64,
    /// When it was made, epoch ms.
    pub created_ms: i64,
    /// When it last changed, epoch ms.
    pub updated_ms: i64,
}

/// One kept event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRow {
    /// Its number within the task.
    pub seq: i64,
    /// `status`, `progress` or `artifact`.
    pub kind: String,
    /// The `StreamResponse`, as JSON.
    pub payload: String,
}

/// A push configuration, as kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushRow {
    /// The task.
    pub task_id: String,
    /// The configuration's id, unique within the task.
    pub id: String,
    /// The webhook.
    pub url: String,
    /// The token the webhook checks, if any.
    pub token: Option<String>,
    /// The `Authorization` scheme, if any.
    pub auth_scheme: Option<String>,
    /// Its credentials, if any.
    pub auth_credentials: Option<String>,
    /// When it was made, epoch ms.
    pub created_ms: i64,
}

/// A task to make, with what it is made in.
#[derive(Debug, Clone)]
pub struct NewTask {
    /// The task.
    pub task: TaskRow,
    /// Its context, when the task opens one.
    pub context: Option<ContextRow>,
    /// The `(caller, messageId)` its first message claims.
    pub message_key: Option<(String, String)>,
    /// Its first event, numbered `task.seq`.
    pub event: EventRow,
}

/// The filters of a task list.
#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    /// Only this entry's.
    pub entry: Option<String>,
    /// Only this caller's.
    pub caller: Option<String>,
    /// Only this context's.
    pub context_id: Option<String>,
    /// Only tasks in this state.
    pub state: Option<TaskState>,
    /// Only tasks whose status changed at or after this, epoch ms.
    pub status_after_ms: Option<i64>,
}

/// The A2A tables.
#[derive(Debug, Clone)]
pub struct Store {
    db: Arc<Db>,
}

const TASK_COLUMNS: &str =
    "id, context_id, entry, caller, state, status_message, status_ms, artifacts, history, client_message_id, dispatched, seq, created_ms, updated_ms";

fn task_at(row: &Row<'_>) -> rusqlite::Result<TaskRow> {
    let state: String = row.get(4)?;
    let message: Option<String> = row.get(5)?;
    let artifacts: String = row.get(7)?;
    let history: String = row.get(8)?;
    Ok(TaskRow {
        id: row.get(0)?,
        context_id: row.get(1)?,
        entry: row.get(2)?,
        caller: row.get(3)?,
        state: TaskState::parse(&state).unwrap_or_default(),
        status_message: message.and_then(|json| serde_json::from_str(&json).ok()),
        status_ms: row.get(6)?,
        artifacts: serde_json::from_str(&artifacts).unwrap_or_default(),
        history: serde_json::from_str(&history).unwrap_or_default(),
        client_message_id: row.get(9)?,
        dispatched: row.get::<_, i64>(10)? != 0,
        seq: row.get(11)?,
        created_ms: row.get(12)?,
        updated_ms: row.get(13)?,
    })
}

fn context_at(row: &Row<'_>) -> rusqlite::Result<ContextRow> {
    let source: String = row.get(3)?;
    Ok(ContextRow {
        context_id: row.get(0)?,
        entry: row.get(1)?,
        caller: row.get(2)?,
        session: SessionRef {
            source: EntrySource::parse(&source).unwrap_or(EntrySource::Claude),
            session_id: row.get(4)?,
            cwd: row.get(5)?,
            branch: row.get(6)?,
        },
        created_ms: row.get(7)?,
    })
}

fn token_at(row: &Row<'_>) -> rusqlite::Result<TokenRow> {
    let entries: String = row.get(2)?;
    Ok(TokenRow {
        id: row.get(0)?,
        name: row.get(1)?,
        entries: serde_json::from_str(&entries).unwrap_or_default(),
        secret_hash: row.get(3)?,
        created_ms: row.get(4)?,
        last_used_ms: row.get(5)?,
        revoked_ms: row.get(6)?,
    })
}

fn push_at(row: &Row<'_>) -> rusqlite::Result<PushRow> {
    Ok(PushRow {
        task_id: row.get(0)?,
        id: row.get(1)?,
        url: row.get(2)?,
        token: row.get(3)?,
        auth_scheme: row.get(4)?,
        auth_credentials: row.get(5)?,
        created_ms: row.get(6)?,
    })
}

fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

/// The terminal states, as the `state` column spells them, for SQL.
fn terminal_states() -> String {
    [TaskState::Completed, TaskState::Failed, TaskState::Canceled, TaskState::Rejected]
        .iter()
        .map(|state| format!("'{}'", state.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Why a task could not be made.
#[derive(Debug)]
pub enum CreateError {
    /// Its `(caller, messageId)` is taken: by this task.
    Duplicate(String),
    /// The database refused.
    Db(rusqlite::Error),
}

impl Store {
    /// The tables of `db`.
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }

    /* ── entries ─────────────────────────────────────────────────────── */

    /// Every entry, oldest first. A row that will not parse is left out.
    pub fn entries(&self) -> Vec<A2aEntry> {
        self.db
            .read(|conn| {
                let mut statement = conn.prepare("SELECT payload FROM a2a_entries ORDER BY created_ms, id")?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                Ok(rows
                    .filter_map(Result::ok)
                    .filter_map(|payload| serde_json::from_str(&payload).ok())
                    .collect())
            })
            .unwrap_or_default()
    }

    /// One entry.
    pub fn entry(&self, id: &str) -> Option<A2aEntry> {
        self.db
            .read(|conn| {
                conn.query_row("SELECT payload FROM a2a_entries WHERE id = ?1", [id], |row| row.get::<_, String>(0))
                    .optional()
            })
            .ok()
            .flatten()
            .and_then(|payload| serde_json::from_str(&payload).ok())
    }

    /// Add an entry; `false` when its id is taken.
    pub fn insert_entry(&self, entry: &A2aEntry, now_ms: i64) -> rusqlite::Result<bool> {
        self.db.write(|tx| {
            let added = tx.execute(
                "INSERT OR IGNORE INTO a2a_entries (id, payload, created_ms, updated_ms) VALUES (?1, ?2, ?3, ?3)",
                params![entry.id, json(entry), now_ms],
            )?;
            Ok(added == 1)
        })
    }

    /// Replace an entry's settings.
    pub fn replace_entry(&self, entry: &A2aEntry, now_ms: i64) -> rusqlite::Result<()> {
        self.db.write(|tx| {
            tx.execute(
                "UPDATE a2a_entries SET payload = ?2, updated_ms = ?3 WHERE id = ?1",
                params![entry.id, json(entry), now_ms],
            )?;
            Ok(())
        })
    }

    /// Remove an entry; `false` when there was none.
    pub fn delete_entry(&self, id: &str) -> rusqlite::Result<bool> {
        self.db.write(|tx| Ok(tx.execute("DELETE FROM a2a_entries WHERE id = ?1", [id])? == 1))
    }

    /* ── tokens ──────────────────────────────────────────────────────── */

    /// Every token, the newest first.
    pub fn tokens(&self) -> Vec<TokenRow> {
        self.db
            .read(|conn| {
                let mut statement = conn.prepare(
                    "SELECT id, name, entries, secret_hash, created_ms, last_used_ms, revoked_ms FROM a2a_tokens ORDER BY created_ms DESC, id",
                )?;
                let rows = statement.query_map([], token_at)?;
                Ok(rows.filter_map(Result::ok).collect())
            })
            .unwrap_or_default()
    }

    /// One token.
    pub fn token(&self, id: &str) -> Option<TokenRow> {
        self.db
            .read(|conn| {
                conn.query_row(
                    "SELECT id, name, entries, secret_hash, created_ms, last_used_ms, revoked_ms FROM a2a_tokens WHERE id = ?1",
                    [id],
                    token_at,
                )
                .optional()
            })
            .ok()
            .flatten()
    }

    /// Keep a new token.
    pub fn insert_token(&self, token: &TokenRow) -> rusqlite::Result<()> {
        self.db.write(|tx| {
            tx.execute(
                "INSERT INTO a2a_tokens (id, name, entries, secret_hash, created_ms, last_used_ms, revoked_ms) VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL)",
                params![token.id, token.name, json(&token.entries), token.secret_hash, token.created_ms],
            )?;
            Ok(())
        })
    }

    /// Revoke a token: `None` when there is none, `Some(true)` when this
    /// revoked it, `Some(false)` when it was revoked already (and keeps the
    /// first revocation's time).
    pub fn revoke_token(&self, id: &str, now_ms: i64) -> rusqlite::Result<Option<bool>> {
        self.db.write(|tx| {
            let revoked = tx.execute(
                "UPDATE a2a_tokens SET revoked_ms = ?2 WHERE id = ?1 AND revoked_ms IS NULL",
                params![id, now_ms],
            )?;
            if revoked == 1 {
                return Ok(Some(true));
            }
            let exists: Option<String> = tx
                .query_row("SELECT id FROM a2a_tokens WHERE id = ?1", [id], |row| row.get(0))
                .optional()?;
            Ok(exists.map(|_| false))
        })
    }

    /// A token proved a request now.
    pub fn touch_token(&self, id: &str, now_ms: i64) {
        self.db.commit("an A2A token's last use", |tx| {
            tx.execute("UPDATE a2a_tokens SET last_used_ms = ?2 WHERE id = ?1", params![id, now_ms])?;
            Ok(())
        });
    }

    /// Take a deleted entry out of every token's scope, so an entry made later
    /// under the same id is not reached by a token made for the old one. Each
    /// token keeps its other entries.
    pub fn drop_entry_from_tokens(&self, entry: &str) -> rusqlite::Result<()> {
        let tokens = self.tokens();
        self.db.write(|tx| {
            for token in tokens.iter().filter(|token| token.entries.iter().any(|e| e == entry)) {
                let kept: Vec<&String> = token.entries.iter().filter(|e| *e != entry).collect();
                tx.execute("UPDATE a2a_tokens SET entries = ?2 WHERE id = ?1", params![token.id, json(&kept)])?;
            }
            Ok(())
        })
    }

    /* ── contexts ────────────────────────────────────────────────────── */

    /// One context.
    pub fn context(&self, id: &str) -> Option<ContextRow> {
        self.db
            .read(|conn| {
                conn.query_row(
                    "SELECT context_id, entry, caller, source, session_id, cwd, branch, created_ms FROM a2a_contexts WHERE context_id = ?1",
                    [id],
                    context_at,
                )
                .optional()
            })
            .ok()
            .flatten()
    }

    /// A context of `caller` on `entry`: another caller's is not found (§6).
    pub fn context_for(&self, entry: &str, caller: &str, id: &str) -> Option<ContextRow> {
        self.db
            .read(|conn| {
                conn.query_row(
                    "SELECT context_id, entry, caller, source, session_id, cwd, branch, created_ms FROM a2a_contexts
                      WHERE context_id = ?1 AND entry = ?2 AND caller = ?3",
                    params![id, entry, caller],
                    context_at,
                )
                .optional()
            })
            .ok()
            .flatten()
    }

    /// How many contexts `caller` opened since `since_ms`.
    pub fn contexts_since(&self, caller: &str, since_ms: i64) -> usize {
        self.db
            .read(|conn| {
                conn.query_row(
                    "SELECT count(*) FROM a2a_contexts WHERE caller = ?1 AND created_ms >= ?2",
                    params![caller, since_ms],
                    |row| row.get::<_, i64>(0),
                )
            })
            .map(|n| n.max(0) as usize)
            .unwrap_or_default()
    }

    /// The context a session belongs to, if an A2A task opened it.
    pub fn context_of_session(&self, source: EntrySource, session_id: &str) -> Option<ContextRow> {
        self.db
            .read(|conn| {
                conn.query_row(
                    "SELECT context_id, entry, caller, source, session_id, cwd, branch, created_ms FROM a2a_contexts
                      WHERE source = ?1 AND session_id = ?2",
                    params![source.as_str(), session_id],
                    context_at,
                )
                .optional()
            })
            .ok()
            .flatten()
    }

    /// What the backend learned of a context's session: its id, its directory
    /// and its branch. A `None` id leaves the known one alone.
    pub fn set_session(&self, context_id: &str, session_id: Option<&str>, cwd: Option<&str>, branch: Option<Option<&str>>) {
        self.db.commit("an A2A context's session", |tx| {
            if let Some(session_id) = session_id {
                tx.execute(
                    "UPDATE a2a_contexts SET session_id = ?2 WHERE context_id = ?1",
                    params![context_id, session_id],
                )?;
            }
            if let Some(cwd) = cwd {
                tx.execute("UPDATE a2a_contexts SET cwd = ?2 WHERE context_id = ?1", params![context_id, cwd])?;
            }
            if let Some(branch) = branch {
                tx.execute("UPDATE a2a_contexts SET branch = ?2 WHERE context_id = ?1", params![context_id, branch])?;
            }
            Ok(())
        });
    }

    /* ── tasks ───────────────────────────────────────────────────────── */

    /// One task.
    pub fn task(&self, id: &str) -> Option<TaskRow> {
        self.db
            .read(|conn| {
                conn.query_row(&format!("SELECT {TASK_COLUMNS} FROM a2a_tasks WHERE id = ?1"), [id], task_at)
                    .optional()
            })
            .ok()
            .flatten()
    }

    /// A task of `caller` on `entry`: another caller's is not found, exactly as
    /// one that does not exist (§6).
    pub fn task_for(&self, entry: &str, caller: &str, id: &str) -> Option<TaskRow> {
        self.db
            .read(|conn| {
                conn.query_row(
                    &format!("SELECT {TASK_COLUMNS} FROM a2a_tasks WHERE id = ?1 AND entry = ?2 AND caller = ?3"),
                    params![id, entry, caller],
                    task_at,
                )
                .optional()
            })
            .ok()
            .flatten()
    }

    /// How many of `caller`'s tasks are queued: submitted, on any entry.
    pub fn queued(&self, caller: &str) -> usize {
        self.db
            .read(|conn| {
                conn.query_row(
                    "SELECT count(*) FROM a2a_tasks WHERE caller = ?1 AND state = ?2",
                    params![caller, TaskState::Submitted.as_str()],
                    |row| row.get::<_, i64>(0),
                )
            })
            .map(|n| n.max(0) as usize)
            .unwrap_or_default()
    }

    /// Make a task, with its context and its message key, in one transaction.
    /// A message key another task holds is [`CreateError::Duplicate`], and
    /// nothing is made.
    pub fn create(&self, new: &NewTask) -> Result<(), CreateError> {
        let outcome = self.db.write(|tx| {
            if let Some((caller, message_id)) = &new.message_key {
                let claimed = tx.execute(
                    "INSERT OR IGNORE INTO a2a_messages (caller, message_id, task_id, created_ms) VALUES (?1, ?2, ?3, ?4)",
                    params![caller, message_id, new.task.id, new.task.created_ms],
                )?;
                if claimed == 0 {
                    let holder: String = tx.query_row(
                        "SELECT task_id FROM a2a_messages WHERE caller = ?1 AND message_id = ?2",
                        params![caller, message_id],
                        |row| row.get(0),
                    )?;
                    return Ok(Some(holder));
                }
            }
            if let Some(context) = &new.context {
                tx.execute(
                    "INSERT INTO a2a_contexts (context_id, entry, caller, source, session_id, cwd, branch, created_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        context.context_id,
                        context.entry,
                        context.caller,
                        context.session.source.as_str(),
                        context.session.session_id,
                        context.session.cwd,
                        context.session.branch,
                        context.created_ms
                    ],
                )?;
            }
            insert_task(tx, &new.task)?;
            insert_event(tx, &new.task.id, &new.event, new.task.created_ms)?;
            Ok(None)
        });
        match outcome {
            Ok(None) => Ok(()),
            Ok(Some(holder)) => Err(CreateError::Duplicate(holder)),
            Err(error) => Err(CreateError::Db(error)),
        }
    }

    /// Write a task's new state and the events its change made.
    pub fn save(&self, task: &TaskRow, events: &[EventRow]) -> rusqlite::Result<()> {
        let prune = task.state.is_terminal() && PRUNE.due(task.updated_ms);
        self.db.write(|tx| {
            tx.execute(
                "UPDATE a2a_tasks SET state = ?2, status_message = ?3, status_ms = ?4, artifacts = ?5, history = ?6,
                        dispatched = ?7, seq = ?8, updated_ms = ?9
                  WHERE id = ?1",
                params![
                    task.id,
                    task.state.as_str(),
                    task.status_message.as_ref().map(json),
                    task.status_ms,
                    json(&task.artifacts),
                    json(&task.history),
                    i64::from(task.dispatched),
                    task.seq,
                    task.updated_ms
                ],
            )?;
            for event in events {
                insert_event(tx, &task.id, event, task.updated_ms)?;
            }
            if prune {
                tx.execute(
                    &format!(
                        "DELETE FROM a2a_task_events WHERE task_id IN
                           (SELECT id FROM a2a_tasks WHERE state IN ({}) AND updated_ms < ?1)",
                        terminal_states()
                    ),
                    [task.updated_ms - EVENTS_KEPT_MS],
                )?;
            }
            Ok(())
        })
    }

    /// Claim `(caller, messageId)` for a message to `task_id`. `Some(holder)`
    /// when it was claimed already, by `holder`.
    pub fn claim_message(&self, caller: &str, message_id: &str, task_id: &str, now_ms: i64) -> rusqlite::Result<Option<String>> {
        self.db.write(|tx| {
            let claimed = tx.execute(
                "INSERT OR IGNORE INTO a2a_messages (caller, message_id, task_id, created_ms) VALUES (?1, ?2, ?3, ?4)",
                params![caller, message_id, task_id, now_ms],
            )?;
            if claimed == 1 {
                return Ok(None);
            }
            tx.query_row(
                "SELECT task_id FROM a2a_messages WHERE caller = ?1 AND message_id = ?2",
                params![caller, message_id],
                |row| row.get(0),
            )
            .map(Some)
        })
    }

    /// Give back a claim whose message did not go through, so a retry may.
    pub fn release_message(&self, caller: &str, message_id: &str) {
        self.db.commit("an A2A message's release", |tx| {
            tx.execute(
                "DELETE FROM a2a_messages WHERE caller = ?1 AND message_id = ?2",
                params![caller, message_id],
            )?;
            Ok(())
        });
    }

    /// The task a `(caller, messageId)` went to.
    pub fn message_task(&self, caller: &str, message_id: &str) -> Option<String> {
        self.db
            .read(|conn| {
                conn.query_row(
                    "SELECT task_id FROM a2a_messages WHERE caller = ?1 AND message_id = ?2",
                    params![caller, message_id],
                    |row| row.get(0),
                )
                .optional()
            })
            .ok()
            .flatten()
    }

    /// The tasks `filter` lets through, the most recently changed first:
    /// `limit` of them after `after` (the `(status_ms, id)` the previous page
    /// ended at), and how many there are in all.
    pub fn list(&self, filter: &TaskFilter, after: Option<(i64, String)>, limit: usize) -> (Vec<TaskRow>, usize) {
        let mut clauses = Vec::new();
        let mut values: Vec<rusqlite::types::Value> = Vec::new();
        let mut push = |clause: &str, value: rusqlite::types::Value| {
            values.push(value);
            clauses.push(clause.replace('?', &format!("?{}", values.len())));
        };
        if let Some(entry) = &filter.entry {
            push("entry = ?", entry.clone().into());
        }
        if let Some(caller) = &filter.caller {
            push("caller = ?", caller.clone().into());
        }
        if let Some(context) = &filter.context_id {
            push("context_id = ?", context.clone().into());
        }
        if let Some(state) = filter.state {
            push("state = ?", state.as_str().to_string().into());
        }
        if let Some(after) = filter.status_after_ms {
            push("status_ms >= ?", after.into());
        }
        let filtered = if clauses.is_empty() { "1".to_string() } else { clauses.join(" AND ") };
        let total_values = values.clone();
        let mut page_clause = filtered.clone();
        if let Some((status_ms, id)) = after {
            values.push(status_ms.into());
            let ms = values.len();
            values.push(id.into());
            let id_at = values.len();
            page_clause = format!("{filtered} AND (status_ms < ?{ms} OR (status_ms = ?{ms} AND id > ?{id_at}))");
        }
        values.push((limit as i64).into());
        let limit_at = values.len();
        self.db
            .read(|conn| {
                let total: i64 = conn.query_row(
                    &format!("SELECT count(*) FROM a2a_tasks WHERE {filtered}"),
                    rusqlite::params_from_iter(total_values.iter()),
                    |row| row.get(0),
                )?;
                let mut statement = conn.prepare(&format!(
                    "SELECT {TASK_COLUMNS} FROM a2a_tasks WHERE {page_clause} ORDER BY status_ms DESC, id LIMIT ?{limit_at}"
                ))?;
                let rows = statement.query_map(rusqlite::params_from_iter(values.iter()), task_at)?;
                Ok((rows.filter_map(Result::ok).collect(), total.max(0) as usize))
            })
            .unwrap_or_default()
    }

    /// Every task not finished yet, oldest first.
    pub fn open_tasks(&self) -> Vec<TaskRow> {
        self.query_tasks(&format!("state NOT IN ({}) ORDER BY created_ms, id", terminal_states()), &[])
    }

    /// The unfinished tasks of a context.
    pub fn open_tasks_of_context(&self, context_id: &str) -> Vec<TaskRow> {
        self.query_tasks(
            &format!("context_id = ?1 AND state NOT IN ({}) ORDER BY created_ms, id", terminal_states()),
            &[context_id],
        )
    }

    /// The unfinished tasks of an entry.
    pub fn open_tasks_of_entry(&self, entry: &str) -> Vec<TaskRow> {
        self.query_tasks(
            &format!("entry = ?1 AND state NOT IN ({}) ORDER BY created_ms, id", terminal_states()),
            &[entry],
        )
    }

    /// The oldest task of an entry still waiting for a slot.
    pub fn next_pending(&self, entry: &str) -> Option<TaskRow> {
        self.query_tasks(
            &format!(
                "entry = ?1 AND dispatched = 0 AND state = '{}' ORDER BY created_ms, id LIMIT 1",
                TaskState::Submitted.as_str()
            ),
            &[entry],
        )
        .into_iter()
        .next()
    }

    /// How many of an entry's tasks hold a slot: handed to the backend and
    /// submitted or working.
    pub fn running(&self, entry: &str) -> usize {
        self.db
            .read(|conn| {
                conn.query_row(
                    "SELECT count(*) FROM a2a_tasks WHERE entry = ?1 AND dispatched = 1 AND state IN (?2, ?3)",
                    params![entry, TaskState::Submitted.as_str(), TaskState::Working.as_str()],
                    |row| row.get::<_, i64>(0),
                )
            })
            .map(|n| n.max(0) as usize)
            .unwrap_or_default()
    }

    /// The first task a context was opened with.
    pub fn first_task_of_context(&self, context_id: &str) -> Option<String> {
        self.db
            .read(|conn| {
                conn.query_row(
                    "SELECT id FROM a2a_tasks WHERE context_id = ?1 ORDER BY created_ms, rowid LIMIT 1",
                    [context_id],
                    |row| row.get(0),
                )
                .optional()
            })
            .ok()
            .flatten()
    }

    fn query_tasks(&self, clause: &str, args: &[&str]) -> Vec<TaskRow> {
        self.db
            .read(|conn| {
                let mut statement = conn.prepare(&format!("SELECT {TASK_COLUMNS} FROM a2a_tasks WHERE {clause}"))?;
                let rows = statement.query_map(rusqlite::params_from_iter(args.iter()), task_at)?;
                Ok(rows.filter_map(Result::ok).collect())
            })
            .unwrap_or_default()
    }

    /// A task's kept events after `seq`, in order.
    pub fn events_after(&self, task_id: &str, seq: i64) -> Vec<EventRow> {
        self.db
            .read(|conn| {
                let mut statement = conn.prepare("SELECT seq, kind, payload FROM a2a_task_events WHERE task_id = ?1 AND seq > ?2 ORDER BY seq")?;
                let rows = statement.query_map(params![task_id, seq], |row| {
                    Ok(EventRow {
                        seq: row.get(0)?,
                        kind: row.get(1)?,
                        payload: row.get(2)?,
                    })
                })?;
                Ok(rows.filter_map(Result::ok).collect())
            })
            .unwrap_or_default()
    }

    /* ── push configurations ─────────────────────────────────────────── */

    /// Keep a push configuration, replacing one of the same id. Its delivery
    /// cursor starts before the task's first event when `from_start` (a
    /// configuration the task's first message came with is told the whole
    /// task), else at the task's newest event, read in the same transaction:
    /// one added later is told what happens from then on, and an event
    /// written after this is never one it was not meant to get.
    pub fn put_push(&self, push: &PushRow, from_start: bool) -> rusqlite::Result<()> {
        self.db.write(|tx| {
            tx.execute(
                "INSERT OR REPLACE INTO a2a_push_configs (task_id, id, url, token, auth_scheme, auth_credentials, created_ms, delivered_seq)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
                         CASE WHEN ?8 THEN 0 ELSE COALESCE((SELECT seq FROM a2a_tasks WHERE id = ?1), 0) END)",
                params![
                    push.task_id,
                    push.id,
                    push.url,
                    push.token,
                    push.auth_scheme,
                    push.auth_credentials,
                    push.created_ms,
                    from_start
                ],
            )?;
            Ok(())
        })
    }

    /// Where a push configuration's delivery stands: the number of the last
    /// event of its task it was told of, or gave up on. `None` when there is
    /// no such configuration.
    pub fn push_cursor(&self, task_id: &str, id: &str) -> Option<i64> {
        self.db
            .read(|conn| {
                conn.query_row(
                    "SELECT delivered_seq FROM a2a_push_configs WHERE task_id = ?1 AND id = ?2",
                    params![task_id, id],
                    |row| row.get(0),
                )
                .optional()
            })
            .ok()
            .flatten()
    }

    /// Move a push configuration's cursor up to `seq`, never back: a
    /// configuration replaced meanwhile keeps the cursor it was given.
    pub fn advance_push(&self, task_id: &str, id: &str, seq: i64) -> rusqlite::Result<()> {
        self.db.write(|tx| {
            tx.execute(
                "UPDATE a2a_push_configs SET delivered_seq = MAX(delivered_seq, ?3) WHERE task_id = ?1 AND id = ?2",
                params![task_id, id, seq],
            )?;
            Ok(())
        })
    }

    /// Every push configuration whose task has events after its cursor, as
    /// `(task, id)`: what a restart has left to deliver.
    pub fn pending_pushes(&self) -> Vec<(String, String)> {
        self.db
            .read(|conn| {
                let mut statement = conn.prepare(
                    "SELECT p.task_id, p.id FROM a2a_push_configs p JOIN a2a_tasks t ON t.id = p.task_id
                      WHERE p.delivered_seq < t.seq ORDER BY p.created_ms, p.task_id, p.id",
                )?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                Ok(rows.filter_map(Result::ok).collect())
            })
            .unwrap_or_default()
    }

    /// One push configuration.
    pub fn push(&self, task_id: &str, id: &str) -> Option<PushRow> {
        self.db
            .read(|conn| {
                conn.query_row(
                    "SELECT task_id, id, url, token, auth_scheme, auth_credentials, created_ms FROM a2a_push_configs WHERE task_id = ?1 AND id = ?2",
                    params![task_id, id],
                    push_at,
                )
                .optional()
            })
            .ok()
            .flatten()
    }

    /// A task's push configurations, oldest first.
    pub fn pushes(&self, task_id: &str) -> Vec<PushRow> {
        self.db
            .read(|conn| {
                let mut statement = conn.prepare(
                    "SELECT task_id, id, url, token, auth_scheme, auth_credentials, created_ms FROM a2a_push_configs WHERE task_id = ?1 ORDER BY created_ms, id",
                )?;
                let rows = statement.query_map([task_id], push_at)?;
                Ok(rows.filter_map(Result::ok).collect())
            })
            .unwrap_or_default()
    }

    /// Remove every push configuration of a task.
    pub fn delete_pushes(&self, task_id: &str) -> rusqlite::Result<()> {
        self.db.write(|tx| {
            tx.execute("DELETE FROM a2a_push_configs WHERE task_id = ?1", [task_id])?;
            Ok(())
        })
    }

    /// Remove a push configuration; nothing to remove is no error.
    pub fn delete_push(&self, task_id: &str, id: &str) -> rusqlite::Result<()> {
        self.db.write(|tx| {
            tx.execute("DELETE FROM a2a_push_configs WHERE task_id = ?1 AND id = ?2", params![task_id, id])?;
            Ok(())
        })
    }
}

fn insert_task(tx: &rusqlite::Transaction<'_>, task: &TaskRow) -> rusqlite::Result<()> {
    tx.execute(
        &format!("INSERT INTO a2a_tasks ({TASK_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"),
        params![
            task.id,
            task.context_id,
            task.entry,
            task.caller,
            task.state.as_str(),
            task.status_message.as_ref().map(json),
            task.status_ms,
            json(&task.artifacts),
            json(&task.history),
            task.client_message_id,
            i64::from(task.dispatched),
            task.seq,
            task.created_ms,
            task.updated_ms
        ],
    )?;
    Ok(())
}

fn insert_event(tx: &rusqlite::Transaction<'_>, task_id: &str, event: &EventRow, at_ms: i64) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO a2a_task_events (task_id, seq, kind, payload, created_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![task_id, event.seq, event.kind, event.payload, at_ms],
    )?;
    Ok(())
}
