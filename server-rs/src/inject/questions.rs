//! `AskUserQuestion`, answered from wherever the user happens to be.
//!
//! # What the CLI does
//!
//! claude only offers the tool to a `-p` process that has somewhere to send a
//! permission prompt. Started with `--permission-prompt-tool stdio`, it writes
//! the call to stdout as a control request and blocks the turn on it — with no
//! timeout at all — until a control response naming the same `request_id`
//! comes back on stdin:
//!
//! ```text
//! {"type":"control_request","request_id":"…","request":{"subtype":"can_use_tool",
//!  "tool_name":"AskUserQuestion","input":{"questions":[…]},"tool_use_id":"toolu_…"}}
//! {"type":"control_response","response":{"subtype":"success","request_id":"…",
//!  "response":{"behavior":"allow","updatedInput":{"questions":[…],"answers":{"<question>":"Blue"}}}}}
//! ```
//!
//! None of this is documented. It was read off claude 2.1.295 driven against a
//! mock API, and every shape it depends on is spelled in this file and nowhere
//! else: [`PermissionRequest::parse`] reads the request, [`allow`] and [`deny`]
//! write the answer, [`control_response`] wraps it. The parts that were found
//! the hard way:
//!
//! * `updatedInput.questions` must be the tool input's array **verbatim** — the
//!   CLI rebuilds the tool call from it, and a missing one fails the call;
//! * `answers` is keyed by the exact question text, and a multi-select answer
//!   is *one* string, the labels joined with `", "` ([`join_choices`]);
//! * a `response` replaces the answers in what the model is told, rather than
//!   joining them — see [`allow`] for what is sent instead;
//! * the CLI validates none of it: an unknown key is dropped, a duplicate or
//!   unknown `request_id` is ignored. The second fact is what makes re-sending
//!   a stored answer after a restart safe ([`Stored::delivery`]).
//!
//! # Everything else that now asks
//!
//! The flag routes *every* prompt the CLI would have shown a terminal user down
//! the same channel — a write outside the workspace, `.git/config`, the plan
//! tools, an ask rule. Under `--permission-mode auto` those were refused without
//! a word, and they still are: the driver answers each one with [`deny`] the
//! moment it arrives, worded by [`permission_denial`]. Nothing here ever grants
//! a permission; the one thing the user is asked is the question the agent
//! asked.
//!
//! # The record
//!
//! One row per question in the instance's `state.db` (`session_questions`),
//! keyed by the CLI's `request_id`. An answer is a compare-and-set out of
//! `pending`, made **before** anything is written to the CLI, so a second
//! device answering the same question finds the row already moved and is told
//! who answered it and when, rather than sending a second answer into a turn
//! that has moved on.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use gotcha::Schematic;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::db::Db;
use crate::openapi_ext::Nullable;
use crate::sessions::util::QuestionEntry;

/// How long a question is remembered once it is no longer waiting.
pub const KEEP_FOR_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// The most questions a session's state carries.
pub const RECENT_CAP: usize = 20;
/// How often the prune runs, at most.
const PRUNE_EVERY_MS: i64 = 10 * 60 * 1000;

/// What a dismissed question tells the agent.
pub const DISMISSED: &str = "The user chose not to answer this question. Use your own judgement and carry on.";

/// What an agent is told when a question it asked was answered by a message
/// sent after its process had already gone, and the process turns out to be
/// still waiting on it after all — see [`crate::inject`]'s replay.
pub const ANSWERED_ELSEWHERE: &str = "The user answered this question in a separate message; read it and carry on.";

/// Where a question is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum QuestionState {
    /// claude is blocked on it, and it can be answered.
    Pending,
    /// The user answered it.
    Answered,
    /// The user declined to answer; the agent was told to use its judgement.
    Dismissed,
    /// The turn it belonged to was stopped, or claude withdrew it.
    Cancelled,
    /// The process that asked it is gone. Still answerable: the answer goes to
    /// the session as an ordinary message instead.
    Expired,
}

impl QuestionState {
    /// The word the table stores.
    pub fn as_str(self) -> &'static str {
        match self {
            QuestionState::Pending => "pending",
            QuestionState::Answered => "answered",
            QuestionState::Dismissed => "dismissed",
            QuestionState::Cancelled => "cancelled",
            QuestionState::Expired => "expired",
        }
    }

    /// A stored word, read back. Anything unrecognised is a question nobody can
    /// do anything with any more, which is what `cancelled` says.
    fn from_stored(stored: &str) -> QuestionState {
        match stored {
            "pending" => QuestionState::Pending,
            "answered" => QuestionState::Answered,
            "dismissed" => QuestionState::Dismissed,
            "expired" => QuestionState::Expired,
            _ => QuestionState::Cancelled,
        }
    }
}

/// One question an agent asked, as a session's state carries it.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SessionQuestion {
    /// The CLI's id for the request, which the answer routes are addressed by.
    pub request_id: String,
    /// The `tool_use` the question is, as the transcript's item names it.
    pub tool_use_id: Nullable<String>,
    /// What was asked.
    pub questions: Vec<QuestionEntry>,
    /// Where it is in its life.
    pub state: QuestionState,
    /// What was chosen, keyed by the exact question text; multi-select answers
    /// are joined with `", "`. `null` until answered.
    pub answers: Nullable<HashMap<String, String>>,
    /// Free text sent with the answer, or instead of one.
    pub response: Nullable<String>,
    /// Who answered or dismissed it, when the server knows; `null` with
    /// authentication off.
    pub answered_by: Nullable<String>,
    /// When it was answered or dismissed, ISO-8601.
    pub answered_at: Nullable<String>,
    /// When claude asked it, ISO-8601.
    pub created_at: String,
}

/// What the user chose, as it is kept and as it is written to the CLI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    /// Question text → the answer, already joined.
    #[serde(default)]
    pub answers: BTreeMap<String, String>,
    /// Question text → a note the user added beside their choice.
    #[serde(default)]
    pub notes: BTreeMap<String, String>,
    /// Free text: the user's own words, with or without a choice.
    #[serde(default)]
    pub response: Option<String>,
}

/// One row of `session_questions`.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    /// The CLI's id for the request.
    pub request_id: String,
    /// The session that asked.
    pub session_id: String,
    /// The `tool_use` the question is.
    pub tool_use_id: Option<String>,
    /// The tool input's `questions` array, verbatim: [`allow`] echoes it.
    pub questions: Value,
    /// Where it is in its life.
    pub state: QuestionState,
    /// What the user chose, once they did.
    pub answer: Option<Answer>,
    /// The permission response written to the CLI for this question, kept so
    /// that a server which died between the compare-and-set and the write can
    /// write it again — the CLI ignores a response it has already had.
    pub delivery: Option<Value>,
    /// Who answered or dismissed it.
    pub answered_by: Option<String>,
    /// When, epoch milliseconds.
    pub answered_ms: Option<i64>,
    /// When claude asked it, epoch milliseconds.
    pub created_ms: i64,
}

impl Stored {
    /// The row in the shape the API speaks.
    pub fn wire(&self) -> SessionQuestion {
        SessionQuestion {
            request_id: self.request_id.clone(),
            tool_use_id: Nullable(self.tool_use_id.clone()),
            questions: entries(&self.questions),
            state: self.state,
            answers: Nullable(
                self.answer
                    .as_ref()
                    .filter(|answer| !answer.answers.is_empty())
                    .map(|answer| answer.answers.clone().into_iter().collect()),
            ),
            response: Nullable(self.answer.as_ref().and_then(|answer| answer.response.clone())),
            answered_by: Nullable(self.answered_by.clone()),
            answered_at: Nullable(self.answered_ms.map(crate::accounts::rest::iso_ms)),
            created_at: crate::accounts::rest::iso_ms(self.created_ms),
        }
    }
}

/// A questions array, in the transcript's own structured form.
fn entries(questions: &Value) -> Vec<QuestionEntry> {
    let mut input = Map::new();
    input.insert("questions".to_string(), questions.clone());
    crate::sessions::tool_display::extract_questions(&input).unwrap_or_default()
}

/// The question store, over the instance database.
#[derive(Debug)]
pub struct Questions {
    db: Arc<Db>,
    /// When the last prune ran.
    prune_gate: crate::db::PruneGate,
}

const COLUMNS: &str = "request_id, session_id, tool_use_id, questions, state, answer, delivery, answered_by, answered_ms, created_ms";

fn stored_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Stored> {
    let json = |text: Option<String>| text.and_then(|text| serde_json::from_str::<Value>(&text).ok());
    Ok(Stored {
        request_id: row.get(0)?,
        session_id: row.get(1)?,
        tool_use_id: row.get(2)?,
        questions: json(Some(row.get(3)?)).unwrap_or(Value::Array(Vec::new())),
        state: QuestionState::from_stored(&row.get::<_, String>(4)?),
        answer: json(row.get(5)?).and_then(|value| serde_json::from_value(value).ok()),
        delivery: json(row.get(6)?),
        answered_by: row.get(7)?,
        answered_ms: row.get(8)?,
        created_ms: row.get(9)?,
    })
}

impl Questions {
    /// The store over an open instance database.
    pub fn new(db: Arc<Db>) -> Self {
        Self {
            db,
            prune_gate: crate::db::PruneGate::new(PRUNE_EVERY_MS),
        }
    }

    /// Write down a question claude has just asked. `true` when it is new —
    /// the one time it is worth telling a phone about — and `false` for one
    /// already on file, which is what a replay after a restart finds.
    pub fn open(&self, request_id: &str, session: &str, tool_use_id: Option<&str>, questions: &Value, now: i64) -> bool {
        self.prune_now_and_then(now);
        let inserted = self.db.write(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO session_questions
                     (request_id, session_id, tool_use_id, questions, state, created_ms, updated_ms)
                 VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?5)",
                rusqlite::params![request_id, session, tool_use_id, questions.to_string(), now],
            )
        });
        match inserted {
            Ok(changed) => changed > 0,
            Err(error) => {
                tracing::warn!(path = %self.db.path().display(), request_id, %error, "cannot record a question");
                false
            }
        }
    }

    /// One question, by the CLI's id for it.
    pub fn get(&self, request_id: &str) -> Option<Stored> {
        self.db
            .read(|conn| {
                conn.query_row(
                    &format!("SELECT {COLUMNS} FROM session_questions WHERE request_id = ?1"),
                    [request_id],
                    stored_row,
                )
                .optional()
            })
            .ok()
            .flatten()
    }

    /// Move a question out of one of `from` into `to`, and only if nobody has
    /// moved it first.
    ///
    /// The whole of "a second device is told it was already answered": one
    /// `UPDATE … WHERE state IN (…)` on the one connection, so of two answers
    /// racing for one question exactly one changes a row. `Ok` carries the row
    /// as it now stands; `Err` carries it as somebody else left it — boxed, it
    /// is the rare arm — or `None` when there is no such question at all.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve(
        &self,
        request_id: &str,
        from: &[QuestionState],
        to: QuestionState,
        answer: Option<&Answer>,
        delivery: Option<&Value>,
        by: Option<&str>,
        now: i64,
    ) -> Result<Stored, Option<Box<Stored>>> {
        // The states are this module's own words, never a caller's, so they are
        // spelled into the statement rather than bound: SQLite has no way to
        // bind a list.
        let from = from.iter().map(|state| format!("'{}'", state.as_str())).collect::<Vec<_>>().join(", ");
        // A person's decision is stamped with who and when; a question that
        // merely ran out (cancelled, expired) keeps whatever it had.
        let decided = matches!(to, QuestionState::Answered | QuestionState::Dismissed);
        let sql = format!(
            "UPDATE session_questions
                SET state = ?2,
                    answer = COALESCE(?3, answer),
                    delivery = COALESCE(?4, delivery),
                    answered_by = CASE WHEN ?6 THEN ?5 ELSE answered_by END,
                    answered_ms = CASE WHEN ?6 THEN ?7 ELSE answered_ms END,
                    updated_ms = ?7
              WHERE request_id = ?1 AND state IN ({from})"
        );
        let answer = answer.and_then(|answer| serde_json::to_string(answer).ok());
        let delivery = delivery.map(Value::to_string);
        let changed = self
            .db
            .write(|tx| tx.execute(&sql, rusqlite::params![request_id, to.as_str(), answer, delivery, by, decided, now]));
        match changed {
            Ok(1) => self.get(request_id).ok_or(None),
            Ok(_) => Err(self.get(request_id).map(Box::new)),
            Err(error) => {
                tracing::warn!(path = %self.db.path().display(), request_id, %error, "cannot record what became of a question");
                Err(self.get(request_id).map(Box::new))
            }
        }
    }

    /// Close a question nobody answered: the turn was stopped, claude withdrew
    /// it, or the process went. `true` when it was still pending.
    pub fn close(&self, request_id: &str, to: QuestionState, now: i64) -> bool {
        self.resolve(request_id, &[QuestionState::Pending], to, None, None, None, now).is_ok()
    }

    /// Take a question back from `from` to `to`, forgetting any answer: an
    /// answer that could not be delivered, or an expired question a re-attached
    /// process turns out to be waiting on after all. `true` when it moved.
    pub fn rewind(&self, request_id: &str, from: QuestionState, to: QuestionState, now: i64) -> bool {
        let rewound = self.db.write(|tx| {
            tx.execute(
                "UPDATE session_questions
                    SET state = ?3, answer = NULL, delivery = NULL, answered_by = NULL, answered_ms = NULL, updated_ms = ?4
                  WHERE request_id = ?1 AND state = ?2",
                rusqlite::params![request_id, from.as_str(), to.as_str(), now],
            )
        });
        matches!(rewound, Ok(1))
    }

    /// The questions still pending for one session, oldest first.
    pub fn pending_for(&self, session: &str) -> Vec<Stored> {
        self.db
            .read(|conn| {
                let mut stmt = conn.prepare_cached(&format!(
                    "SELECT {COLUMNS} FROM session_questions
                      WHERE session_id = ?1 AND state = 'pending' ORDER BY created_ms, request_id"
                ))?;
                let rows = stmt.query_map([session], stored_row)?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default()
    }

    /// Expire every pending question whose session has no process behind it in
    /// `held`. Run once a boot has re-attached to what survived it: a question
    /// whose process did not survive can no longer be answered in its turn.
    pub fn expire_unheld(&self, held: &HashSet<String>, now: i64) -> usize {
        let pending: Vec<(String, String)> = self
            .db
            .read(|conn| {
                let mut stmt = conn.prepare("SELECT request_id, session_id FROM session_questions WHERE state = 'pending'")?;
                let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default();
        pending
            .into_iter()
            .filter(|(_, session)| !held.contains(session))
            .filter(|(request_id, _)| self.close(request_id, QuestionState::Expired, now))
            .count()
    }

    /// The questions a session's state carries: the newest [`RECENT_CAP`] of
    /// the last [`KEEP_FOR_MS`], oldest first.
    pub fn for_session(&self, session: &str, now: i64) -> Vec<SessionQuestion> {
        let read = self.db.read(|conn| {
            let mut stmt = conn.prepare_cached(&format!(
                "SELECT {COLUMNS} FROM session_questions
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
                tracing::warn!(path = %self.db.path().display(), %error, "cannot read a session's questions");
                Vec::new()
            }
        }
    }

    /// Forget every settled question older than [`KEEP_FOR_MS`]. A pending one
    /// is kept however old it is: something is still waiting on it.
    pub fn prune(&self, now: i64) {
        self.db.commit("the question prune", |tx| {
            tx.execute(
                "DELETE FROM session_questions WHERE created_ms < ?1 AND state != 'pending'",
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

// ------------------------------------------------------------ the protocol

/// A `can_use_tool` control request: claude asking whether it may run a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    /// What the response has to name.
    pub request_id: String,
    /// The tool it wants. `AskUserQuestion` is a question; everything else is
    /// a permission prompt, and refused.
    pub tool_name: String,
    /// The `tool_use` the request is about.
    pub tool_use_id: Option<String>,
    /// The tool's input, as the model wrote it.
    pub input: Value,
    /// Why the CLI is asking, in its own words, when it says.
    pub reason: Option<String>,
}

/// The tool that is a question rather than a permission.
pub const QUESTION_TOOL: &str = "AskUserQuestion";

impl PermissionRequest {
    /// Read one stdout line already parsed as a `control_request`; `None` for
    /// any other request, or one too malformed to answer.
    pub fn parse(line: &Value) -> Option<PermissionRequest> {
        let request = line.get("request")?;
        if request.get("subtype").and_then(Value::as_str) != Some("can_use_tool") {
            return None;
        }
        let text = |value: &Value| -> Option<String> {
            match value {
                Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
                // Tolerated in the shape the SDK types describe it: an object
                // naming the rule or mode that decided.
                Value::Object(object) => ["reason", "type"]
                    .iter()
                    .find_map(|key| object.get(*key).and_then(Value::as_str))
                    .map(str::to_owned),
                _ => None,
            }
        };
        Some(PermissionRequest {
            request_id: line.get("request_id").and_then(Value::as_str)?.to_string(),
            tool_name: request.get("tool_name").and_then(Value::as_str).unwrap_or_default().to_string(),
            tool_use_id: request.get("tool_use_id").and_then(Value::as_str).map(str::to_owned),
            input: request.get("input").cloned().unwrap_or(Value::Null),
            reason: ["decision_reason", "blocked_path"]
                .iter()
                .find_map(|key| request.get(*key).and_then(text)),
        })
    }

    /// Whether this is the agent asking the user something.
    pub fn is_question(&self) -> bool {
        self.tool_name == QUESTION_TOOL
    }

    /// The questions array the answer has to echo, verbatim.
    pub fn questions(&self) -> Value {
        self.input.get("questions").cloned().unwrap_or(Value::Array(Vec::new()))
    }
}

/// The permission response that answers a question.
///
/// What claude 2.1.295 makes of it, read back off the tool result it hands the
/// model: `answers` and `annotations` become `The user answered: "<question>"=
/// "<answer>" notes: <note>, …`, while a `response` becomes `The user
/// responded: <text>` — **instead of** the answers, which are then dropped
/// without a word. So `response` is only ever sent on its own, for words the
/// user typed in place of a choice (a message typed into the composer while the
/// question is open). Typed *beside* a choice, the same words ride as a note on
/// the last question answered, where the model reads them with the choice.
pub fn allow(questions: &Value, answer: &Answer) -> Value {
    let response = answer.response.as_deref().map(str::trim).filter(|response| !response.is_empty());
    let mut notes = answer.notes.clone();
    if let (Some(response), Some(last)) = (response, last_answered(questions, answer)) {
        let note = notes.entry(last).or_default();
        *note = if note.trim().is_empty() {
            response.to_string()
        } else {
            format!("{}; {response}", note.trim())
        };
    }
    let mut input = Map::new();
    input.insert("questions".to_string(), questions.clone());
    input.insert("answers".to_string(), json!(answer.answers));
    if !notes.is_empty() {
        let annotations: Map<String, Value> = notes
            .iter()
            .map(|(question, notes)| (question.clone(), json!({ "notes": notes })))
            .collect();
        input.insert("annotations".to_string(), Value::Object(annotations));
    }
    if answer.answers.is_empty() {
        if let Some(response) = response {
            input.insert("response".to_string(), json!(response));
        }
    }
    json!({ "behavior": "allow", "updatedInput": input })
}

/// The last question, in the order they were asked, that has an answer.
fn last_answered(questions: &Value, answer: &Answer) -> Option<String> {
    question_texts(questions)
        .into_iter()
        .rev()
        .find(|question| answer.answers.contains_key(question))
        .or_else(|| answer.answers.keys().next_back().cloned())
}

/// The permission response that refuses, with what the agent is told.
pub fn deny(message: &str) -> Value {
    json!({ "behavior": "deny", "message": message })
}

/// One line of stdin answering `request_id`.
pub fn control_response(request_id: &str, response: &Value) -> String {
    json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": response },
    })
    .to_string()
}

/// One line of stdin refusing a control request this server does not handle,
/// so the CLI is not left waiting on an answer that will never come.
pub fn control_error(request_id: &str, error: &str) -> String {
    json!({
        "type": "control_response",
        "response": { "subtype": "error", "request_id": request_id, "error": error },
    })
    .to_string()
}

/// What a refused permission prompt tells the agent.
///
/// Worded so that the model reads it as a refusal it should route around — as
/// it read the CLI's own refusal before the prompt tool was switched on — and
/// so that a person reading the transcript can tell who refused it.
pub fn permission_denial(request: &PermissionRequest) -> String {
    let tool = if request.tool_name.is_empty() {
        "this tool"
    } else {
        request.tool_name.as_str()
    };
    match request.reason.as_deref() {
        Some(reason) => format!(
            "Permission to use {tool} was denied ({reason}). This session runs under Droppen, \
             which does not grant interactive permissions; do it another way or ask the user to."
        ),
        None => format!(
            "Permission to use {tool} was denied. This session runs under Droppen, which does \
             not grant interactive permissions; do it another way or ask the user to."
        ),
    }
}

/// A multi-select answer, as the CLI takes it: one string.
pub fn join_choices(values: &[String]) -> String {
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The question texts a questions array asks, in order.
pub fn question_texts(questions: &Value) -> Vec<String> {
    questions
        .as_array()
        .map(|questions| {
            questions
                .iter()
                .filter_map(|question| question.get("question").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// One line saying what was asked, for a notification and a log: the first
/// question, and how many more came with it.
pub fn headline(questions: &Value) -> String {
    let asked = question_texts(questions);
    match asked.as_slice() {
        [] => "a question".to_string(),
        [one] => one.clone(),
        [first, rest @ ..] => format!("{first} (+{} more)", rest.len()),
    }
}

/// The most an answer may say, in characters, counted over its choices, its
/// notes and its free text together: the bound every prompt has
/// (`validate_text`), because every word of it reaches the CLI.
pub const MAX_ANSWER_CHARS: usize = 100_000;

/// Refuse an answer too long to hand the CLI.
pub fn check_answer_size(answer: &Answer) -> Result<(), String> {
    let said: usize = answer
        .answers
        .values()
        .chain(answer.notes.values())
        .chain(answer.response.iter())
        .map(|text| text.chars().count())
        .sum();
    if said > MAX_ANSWER_CHARS {
        return Err(format!("an answer is at most {MAX_ANSWER_CHARS} characters"));
    }
    Ok(())
}

/// Refuse an answer the question could not have been given: a key that is not
/// one of its questions, or nothing at all.
pub fn check_answer(questions: &Value, answer: &Answer) -> Result<(), String> {
    let asked = question_texts(questions);
    for key in answer.answers.keys().chain(answer.notes.keys()) {
        if !asked.iter().any(|question| question == key) {
            return Err(format!("not one of the questions asked: {key}"));
        }
    }
    let answered = answer.answers.values().any(|value| !value.trim().is_empty());
    let said = answer.response.as_deref().is_some_and(|response| !response.trim().is_empty());
    if !answered && !said {
        return Err("an answer needs a choice or some text".to_string());
    }
    Ok(())
}

/// An answer to a question whose process is gone, as an ordinary message.
///
/// The turn that asked is over — `--resume` closes the call with an
/// interruption and never asks again — so the answer can only arrive as the
/// user's next message, and it has to say what it is answering.
pub fn as_message(questions: &Value, answer: &Answer) -> String {
    let asked = question_texts(questions);
    let lines: Vec<String> = asked
        .iter()
        .filter_map(|question| {
            let value = answer.answers.get(question).filter(|value| !value.trim().is_empty())?;
            Some(match answer.notes.get(question).filter(|note| !note.trim().is_empty()) {
                Some(note) => format!("\"{question}\": {value} ({note})"),
                None => format!("\"{question}\": {value}"),
            })
        })
        .collect();
    let response = answer.response.as_deref().map(str::trim).filter(|response| !response.is_empty());
    let mut text = match (lines.as_slice(), response) {
        ([one], _) => format!("Answer to your question {one}"),
        ([], Some(response)) => {
            let question = asked.first().map(String::as_str).unwrap_or("your question");
            return format!("Answer to your question \"{question}\": {response}");
        }
        ([], None) => "Answer to your question.".to_string(),
        (many, _) => format!(
            "Answers to your questions:\n{}",
            many.iter().map(|line| format!("- {line}")).collect::<Vec<_>>().join("\n")
        ),
    };
    if let Some(response) = response {
        text.push_str("\n\n");
        text.push_str(response);
    }
    text
}

/// The `tool_use` ids a stream `user` line carries results for.
///
/// How a question is seen answered on stdout: the CLI echoes the tool result it
/// built from the response as an ordinary `user` event, so a replay that meets
/// one knows the question it answers is no longer open.
pub fn answered_tool_uses(line: &Value) -> Vec<String> {
    if line.get("type").and_then(Value::as_str) != Some("user") {
        return Vec::new();
    }
    line.pointer("/message/content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
                .filter_map(|block| block.get("tool_use_id").and_then(Value::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Epoch milliseconds, the clock every column here is kept in.
pub use crate::time::now_ms;

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "11111111-1111-4111-8111-111111111111";

    fn store() -> (tempfile::TempDir, Questions) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::at(dir.path().join("state.db"));
        (dir, Questions::new(db))
    }

    fn colours() -> Value {
        json!([
            {"question": "Which colour do you prefer?", "header": "Colour",
             "options": [{"label": "Red", "description": "Warm"}, {"label": "Blue", "description": "Cool"}],
             "multiSelect": false},
            {"question": "Which toppings?", "header": "Toppings",
             "options": [{"label": "Cheese"}, {"label": "Olives"}, {"label": "Basil"}],
             "multiSelect": true}
        ])
    }

    /// The exact line the CLI writes, read back into everything the driver
    /// acts on.
    #[test]
    fn a_can_use_tool_request_is_read_with_everything_the_answer_needs() {
        let line: Value = serde_json::from_str(
            r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","display_name":"AskUserQuestion","input":{"questions":[{"question":"Which colour do you prefer?","header":"Colour","options":[{"label":"Red","description":"Warm"},{"label":"Blue","description":"Cool"}],"multiSelect":false}]},"tool_use_id":"toolu_1","requires_user_interaction":true}}"#,
        )
        .expect("json");
        let request = PermissionRequest::parse(&line).expect("a permission request");
        assert_eq!(request.request_id, "req-1");
        assert!(request.is_question());
        assert_eq!(request.tool_use_id.as_deref(), Some("toolu_1"));
        assert_eq!(question_texts(&request.questions()), vec!["Which colour do you prefer?".to_string()]);

        let other: Value = serde_json::from_str(
            r#"{"type":"control_request","request_id":"req-2","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"file_path":"/etc/x"},"decision_reason":"outside the workspace"}}"#,
        )
        .expect("json");
        let other = PermissionRequest::parse(&other).expect("a permission request");
        assert!(!other.is_question());
        assert_eq!(other.reason.as_deref(), Some("outside the workspace"));
        assert!(permission_denial(&other).contains("Permission to use Write was denied (outside the workspace)"));
        assert!(permission_denial(&other).contains("Droppen"));

        let interrupt: Value =
            serde_json::from_str(r#"{"type":"control_request","request_id":"x","request":{"subtype":"interrupt"}}"#).expect("json");
        assert_eq!(PermissionRequest::parse(&interrupt), None, "only can_use_tool is a permission request");
    }

    /// The answer the CLI takes: the questions echoed verbatim, the answers
    /// keyed by the exact question text, a multi-select joined with ", ".
    #[test]
    fn an_answer_echoes_the_questions_and_keys_by_their_exact_text() {
        let questions = colours();
        let answer = Answer {
            answers: BTreeMap::from([
                ("Which colour do you prefer?".to_string(), "Blue".to_string()),
                ("Which toppings?".to_string(), join_choices(&["Cheese".into(), "Olives".into()])),
            ]),
            notes: BTreeMap::new(),
            response: None,
        };
        let response = allow(&questions, &answer);
        assert_eq!(response["behavior"], "allow");
        assert_eq!(
            response["updatedInput"]["questions"], questions,
            "echoed verbatim — the CLI rebuilds the call from it"
        );
        assert_eq!(response["updatedInput"]["answers"]["Which colour do you prefer?"], "Blue");
        assert_eq!(response["updatedInput"]["answers"]["Which toppings?"], "Cheese, Olives");
        assert!(response["updatedInput"].get("response").is_none(), "nothing said, nothing sent");
        assert!(response["updatedInput"].get("annotations").is_none());

        let line: Value = serde_json::from_str(&control_response("req-1", &response)).expect("one line of json");
        assert_eq!(line["type"], "control_response");
        assert_eq!(line["response"]["subtype"], "success");
        assert_eq!(line["response"]["request_id"], "req-1");
        assert_eq!(line["response"]["response"], response);
    }

    /// "Other": the user's own words, as the answer and beside it — and never
    /// as a `response` beside a choice, because claude 2.1.295 then reports
    /// the response *instead of* the answers.
    #[test]
    fn free_text_notes_and_a_response_ride_along() {
        let questions = colours();
        let answer = Answer {
            answers: BTreeMap::from([("Which colour do you prefer?".to_string(), "Teal, if you have it".to_string())]),
            notes: BTreeMap::from([("Which colour do you prefer?".to_string(), "for the header only".to_string())]),
            response: Some("and make it quick".to_string()),
        };
        let response = allow(&questions, &answer);
        assert_eq!(response["updatedInput"]["answers"]["Which colour do you prefer?"], "Teal, if you have it");
        assert_eq!(
            response["updatedInput"]["annotations"]["Which colour do you prefer?"]["notes"], "for the header only; and make it quick",
            "the words beside a choice ride with it"
        );
        assert!(response["updatedInput"].get("response").is_none(), "a response would drop the answers");

        // Beside a choice with no note, the words become its note: on the last
        // question answered, in the order they were asked.
        let both = Answer {
            answers: BTreeMap::from([
                ("Which colour do you prefer?".to_string(), "Blue".to_string()),
                ("Which toppings?".to_string(), "Basil".to_string()),
            ]),
            response: Some("thanks".to_string()),
            ..Answer::default()
        };
        let response = allow(&questions, &both);
        assert_eq!(response["updatedInput"]["annotations"]["Which toppings?"]["notes"], "thanks");
        assert!(response["updatedInput"]["annotations"].get("Which colour do you prefer?").is_none());

        // Instead of a choice — the composer's message while the question is
        // open — the words are the response, and there is nothing to drop.
        let said = Answer {
            response: Some("  just pick one  ".to_string()),
            ..Answer::default()
        };
        let response = allow(&questions, &said);
        assert_eq!(response["updatedInput"]["response"], "just pick one");
        assert_eq!(response["updatedInput"]["answers"], json!({}));
        assert_eq!(response["updatedInput"]["questions"], questions);

        assert_eq!(deny(DISMISSED), json!({"behavior": "deny", "message": DISMISSED}));
        assert_eq!(
            join_choices(&[" a ".into(), "".into(), "b".into()]),
            "a, b",
            "blank choices are not choices"
        );
    }

    #[test]
    fn an_answer_must_answer_what_was_asked() {
        let questions = colours();
        let keyed = |key: &str| Answer {
            answers: BTreeMap::from([(key.to_string(), "Blue".to_string())]),
            ..Answer::default()
        };
        assert!(check_answer(&questions, &keyed("Which colour do you prefer?")).is_ok());
        assert!(
            check_answer(&questions, &keyed("which colour do you prefer?")).is_err(),
            "keyed by the exact text"
        );
        assert!(check_answer(&questions, &Answer::default()).is_err(), "an empty answer is not one");
        assert!(check_answer(
            &questions,
            &Answer {
                response: Some("just pick one".into()),
                ..Answer::default()
            }
        )
        .is_ok());
    }

    #[test]
    fn an_answer_is_bounded_like_a_prompt_over_everything_it_says() {
        let at_the_bound = Answer {
            answers: BTreeMap::from([("Which colour do you prefer?".to_string(), "x".repeat(MAX_ANSWER_CHARS - 10))]),
            notes: BTreeMap::from([("Which colour do you prefer?".to_string(), "y".repeat(5))]),
            response: Some("z".repeat(5)),
        };
        assert!(check_answer_size(&at_the_bound).is_ok());
        let over = Answer {
            response: Some("z".repeat(6)),
            ..at_the_bound
        };
        assert!(check_answer_size(&over).is_err(), "counted together, not per field");
        assert!(
            check_answer_size(&Answer {
                response: Some("日".repeat(MAX_ANSWER_CHARS)),
                ..Answer::default()
            })
            .is_ok(),
            "characters, not bytes"
        );
    }

    /// The fallback once the process that asked is gone.
    #[test]
    fn an_answer_to_an_expired_question_reads_as_a_message_that_says_what_it_answers() {
        let questions = colours();
        let one = Answer {
            answers: BTreeMap::from([("Which colour do you prefer?".to_string(), "Blue".to_string())]),
            ..Answer::default()
        };
        assert_eq!(
            as_message(&questions, &one),
            "Answer to your question \"Which colour do you prefer?\": Blue"
        );
        let both = Answer {
            answers: BTreeMap::from([
                ("Which colour do you prefer?".to_string(), "Blue".to_string()),
                ("Which toppings?".to_string(), "Cheese, Olives".to_string()),
            ]),
            response: Some("thanks".into()),
            ..Answer::default()
        };
        assert_eq!(
            as_message(&questions, &both),
            "Answers to your questions:\n- \"Which colour do you prefer?\": Blue\n- \"Which toppings?\": Cheese, Olives\n\nthanks"
        );
        let said = Answer {
            response: Some("whatever you think".into()),
            ..Answer::default()
        };
        assert_eq!(
            as_message(&questions, &said),
            "Answer to your question \"Which colour do you prefer?\": whatever you think"
        );
    }

    /// Two devices, one question: the second is told who got there first.
    #[test]
    fn a_question_is_answered_once_and_the_second_answer_finds_the_first() {
        let (_dir, store) = store();
        let now = now_ms();
        assert!(store.open("req-1", SESSION, Some("toolu_1"), &colours(), now));
        assert!(
            !store.open("req-1", SESSION, Some("toolu_1"), &colours(), now),
            "a replay finds it on file"
        );

        let answer = Answer {
            answers: BTreeMap::from([("Which colour do you prefer?".to_string(), "Blue".to_string())]),
            ..Answer::default()
        };
        let delivery = allow(&colours(), &answer);
        let first = store
            .resolve(
                "req-1",
                &[QuestionState::Pending],
                QuestionState::Answered,
                Some(&answer),
                Some(&delivery),
                Some("phone"),
                now + 5,
            )
            .expect("the first answer wins");
        assert_eq!(first.state, QuestionState::Answered);
        assert_eq!(first.answered_by.as_deref(), Some("phone"));
        assert_eq!(first.answered_ms, Some(now + 5));
        assert_eq!(first.delivery, Some(delivery));

        let second = store
            .resolve(
                "req-1",
                &[QuestionState::Pending],
                QuestionState::Answered,
                Some(&answer),
                None,
                Some("laptop"),
                now + 9,
            )
            .expect_err("the second finds it answered");
        let second = second.expect("and is told by whom");
        assert_eq!(second.answered_by.as_deref(), Some("phone"));
        assert!(
            !store.close("req-1", QuestionState::Cancelled, now + 10),
            "an answered question is not cancelled after the fact"
        );
        assert!(store
            .resolve("nope", &[QuestionState::Pending], QuestionState::Answered, None, None, None, now)
            .unwrap_err()
            .is_none());

        let wire = store.for_session(SESSION, now + 10);
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0].state, QuestionState::Answered);
        assert_eq!(
            wire[0]
                .answers
                .0
                .as_ref()
                .and_then(|answers| answers.get("Which colour do you prefer?"))
                .map(String::as_str),
            Some("Blue")
        );
        assert_eq!(wire[0].questions.len(), 2);
        assert_eq!(wire[0].tool_use_id.0.as_deref(), Some("toolu_1"));
    }

    #[test]
    fn a_question_whose_process_did_not_survive_expires_and_one_that_did_stays() {
        let (_dir, store) = store();
        let now = now_ms();
        store.open("held", SESSION, None, &colours(), now);
        store.open("orphan", "22222222-2222-4222-8222-222222222222", None, &colours(), now);
        assert_eq!(store.expire_unheld(&HashSet::from([SESSION.to_string()]), now + 1), 1);
        assert_eq!(store.get("held").map(|q| q.state), Some(QuestionState::Pending));
        assert_eq!(store.get("orphan").map(|q| q.state), Some(QuestionState::Expired));
        assert_eq!(store.pending_for(SESSION).len(), 1);

        // Expired is answerable, once: the answer becomes a message.
        assert!(store
            .resolve(
                "orphan",
                &[QuestionState::Expired],
                QuestionState::Answered,
                Some(&Answer::default()),
                None,
                None,
                now + 2
            )
            .is_ok());
        assert!(store
            .resolve(
                "orphan",
                &[QuestionState::Expired],
                QuestionState::Answered,
                Some(&Answer::default()),
                None,
                None,
                now + 3
            )
            .is_err());
        // An answer that could not be written goes back to waiting.
        assert!(store.rewind("orphan", QuestionState::Answered, QuestionState::Pending, now + 4));
        assert!(!store.rewind("orphan", QuestionState::Answered, QuestionState::Pending, now + 4), "once");
        let back = store.get("orphan").expect("row");
        assert_eq!((back.state, back.answer, back.answered_by), (QuestionState::Pending, None, None));
    }

    #[test]
    fn settled_questions_older_than_a_week_are_pruned_and_pending_ones_never_are() {
        let (_dir, store) = store();
        let now = now_ms();
        let then = now - KEEP_FOR_MS - 1;
        store.open("old-answered", SESSION, None, &colours(), then);
        store.close("old-answered", QuestionState::Cancelled, then);
        store.open("old-pending", SESSION, None, &colours(), then);
        store.prune(now);
        assert!(store.get("old-answered").is_none());
        assert!(store.get("old-pending").is_some());
        assert_eq!(store.for_session(SESSION, now).len(), 1, "a pending question is shown however old it is");
    }

    #[test]
    fn a_stream_user_line_names_the_calls_it_answers() {
        let line: Value = serde_json::from_str(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"User has answered"}]}}"#,
        )
        .expect("json");
        assert_eq!(answered_tool_uses(&line), vec!["toolu_1".to_string()]);
        assert!(answered_tool_uses(&json!({"type": "assistant"})).is_empty());
    }
}
