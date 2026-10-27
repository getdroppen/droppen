//! Session discovery and the shapes the API speaks in.
//!
//! Mirrors the former TypeScript server's `schemas.ts` plus the discovery halves
//! of its `claude.ts` and `session-feed.ts`.

pub mod adoption_leftovers;
pub mod checklist;
pub mod claude;
pub mod codex;
pub mod codex_claims;
pub mod codex_tools;
pub mod context;
pub mod continuation;
pub mod mentions;
pub mod owned_sessions;
pub mod pins;
pub mod reference;
pub mod repo;
pub mod subagents;
pub mod tool_display;
pub mod transcript;
pub mod util;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gotcha::Schematic;
use serde::{Deserialize, Serialize};

use crate::accounts::AccountRef;
use crate::claude_roots::{self, ClaudeRoot};
use crate::openapi_ext::{Nullable, Optional};
pub use reference::{ServerId, SessionRef};
use util::TranscriptItem;

/// Which CLI a session belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Claude Code.
    Claude,
    /// OpenAI Codex.
    Codex,
}

impl Source {
    /// The wire name, as it appears in a path.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::Codex => "codex",
        }
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a session is doing anything right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    /// No live CLI process is attached.
    Idle,
    /// A live CLI process is working.
    Running,
    /// A live CLI process is waiting on the user.
    Waiting,
}

/// Whether anything is in flight for a session, in one word.
///
/// [`SessionStatus`] is a *guess*: it comes from a live-process registration
/// plus a heuristic over the tail of the transcript, and both halves are wrong
/// in the cases that matter most — a turn whose recent writing is all
/// sidechain reads as `waiting`, and a message queued behind a turn is
/// invisible to it entirely. This is the answer the driver gives, and where
/// this server holds the process it is not a guess at all.
///
/// Deliberately one word rather than a set of flags. The question a list row
/// has to answer is "is this session doing something, and if not, why is my
/// message not moving" — four answers cover it, and the detail is on
/// [`ResidentInfo`] for whoever wants it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum SessionActivity {
    /// A turn is in flight — or a process is starting to run one.
    Working,
    /// No turn is in flight, but messages are waiting to go in. Either the
    /// session is between turns for a moment, or a settings change is parked
    /// behind background work; [`ResidentInfo::awaiting_background`] says which.
    Queued,
    /// Nothing is in flight and nothing is waiting, but claude is still minding
    /// background tasks a finished turn left running.
    Background,
    /// Nothing at all.
    Idle,
}

impl SessionActivity {
    /// The best answer available with no driver to ask.
    ///
    /// What every row starts as: an external session, a session some other
    /// server drives, a codex thread nobody is injecting into. The transcript
    /// heuristic is all there is, so `running` is taken at its word and
    /// everything else reads as idle — `waiting` in particular is *not* work.
    pub fn from_status(status: SessionStatus) -> Self {
        match status {
            SessionStatus::Running => SessionActivity::Working,
            SessionStatus::Idle | SessionStatus::Waiting => SessionActivity::Idle,
        }
    }
}

/// Where a resident process is in its cycle.
///
/// The wire form of `crate::inject::resident::Phase`. A separate type because
/// the driver's is an internal enum that the injection subsystem is free to
/// grow states on, and this one is a contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum ResidentPhase {
    /// Spawned, but the CLI has not said it is listening yet.
    Starting,
    /// A turn is in flight.
    Working,
    /// Between turns. The process is alive and costs nothing.
    Idle,
}

/// One message a resident has been given and has not answered yet.
///
/// The queue used to be a number, which is enough to say "something is waiting"
/// and not enough to do anything about it: a user who queued the wrong message
/// behind a half-hour turn could neither see what they had sent nor take it
/// back. Each entry carries the id the two queue routes take —
/// `DELETE /api/sessions/{source}/{id}/queue/{qid}` and
/// `POST /api/sessions/{source}/{id}/queue/{qid}/steer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct QueuedMessage {
    /// Stable for as long as the message waits, and across a restart: it is
    /// written to the journal with the message. Never reused.
    pub id: String,
    /// The prompt, cut to a preview length with an ellipsis where it was cut.
    ///
    /// A preview rather than the whole thing because this rides on every list
    /// row: the transcript is where a prompt is read in full, and a queue of
    /// hundred-thousand-character messages would otherwise be sent to a phone
    /// once a second.
    pub text: String,
    /// Whether this one has already been written to claude's stdin, ahead of
    /// its turn, by a steer.
    ///
    /// It is still counted in [`ResidentInfo::queued`] because it has not run
    /// yet — but it is past the point of being taken back, so a client should
    /// offer neither Delete nor Steer on it. Both answer `409`.
    pub steered: bool,
}

/// One piece of work claude started and left running behind its turn.
///
/// The stream says far more about these than a count can carry: every
/// `system:task_started` names the task and says what kind it is, and every
/// `system:background_tasks_changed` republishes the whole live list with the
/// same fields. Keeping only the count meant a session could say "3 background
/// tasks" and nothing else — not what they are, not when they began — while the
/// CLI had been describing each of them all along.
///
/// Everything but the id is optional because the CLI's own shape is: a snapshot
/// entry may carry a `task_type` and no `description`, and this parses
/// tolerantly rather than dropping a task it cannot fully describe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundTask {
    /// The CLI's own `task_id`, stable for the task's life.
    pub id: String,
    /// The id of the `tool_use` that launched it, when the CLI said which.
    ///
    /// The join key between this list and the transcript. A client holding both
    /// can only pair them by this: the transcript knows what the child was
    /// handed — the prompt, the command — and this knows whether it is *still
    /// running*, which the transcript cannot say for a child that reports
    /// through its own channel. Without the pairing a client has to choose one
    /// source and be wrong the other way.
    ///
    /// Null for a task this server only ever heard about in a
    /// `background_tasks_changed` snapshot, which carries no `tool_use_id` —
    /// most often after a restart, when the whole list is reconstructed from
    /// one. Such a task is still worth listing; it just cannot be joined.
    pub tool_use_id: Nullable<String>,
    /// What claude said the task is doing, when it said anything.
    pub description: Nullable<String>,
    /// The CLI's `task_type` — `local_agent`, `local_bash`, whatever it adds
    /// next. Passed through rather than parsed into an enum, because a value
    /// this server has never heard of is still worth showing.
    pub task_type: Nullable<String>,
    /// When this server first heard of the task, by its own clock.
    ///
    /// Not the CLI's start time, which the stream does not carry: after a
    /// restart the whole list is reconstructed from one snapshot, and every task
    /// in it is first seen then. Honest about what it is — the moment remote-hand
    /// learned of the task — rather than precise about something it cannot know.
    pub started_at: Nullable<String>,
    /// Which agent type is running it, for a `local_agent` task.
    ///
    /// The five fields below all come from `system:task_progress`, which the CLI
    /// republishes every few seconds for every live task and which this server
    /// used to step over: a background panel could say "Scout the codebase" and
    /// a stopwatch, while the stream had been reporting the tool it was in, its
    /// token spend and its tool count the whole time. Absent — rather than
    /// null — until the first progress line for that task arrives, so a client
    /// can tell "no progress yet" from "progress said nothing".
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub subagent_type: Optional<String>,
    /// The tool the task was last seen using — `Bash`, `Read`, `Edit`.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub last_tool_name: Optional<String>,
    /// Tokens the task has spent so far, as the CLI counts them.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub total_tokens: Optional<u64>,
    /// How many tools it has run so far.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub tool_uses: Optional<u64>,
    /// How long it has been running, in milliseconds, by the CLI's clock.
    ///
    /// The CLI's own duration rather than `startedAt` arithmetic, which is wrong
    /// by however long this server was down or attached late.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub duration_ms: Optional<u64>,
}

/// What this server's resident CLI says about a session.
///
/// Present exactly when *this* server holds a live process for the session, and
/// absent for external sessions and legacy one-shot runs. A client that finds it here should prefer it to
/// every other signal on the row: it is the process' own account of itself,
/// not an inference from the file it writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ResidentInfo {
    /// Where the process is in its cycle.
    pub phase: ResidentPhase,
    /// How many messages are waiting behind whatever is in flight.
    ///
    /// The field the composer was missing: a send that is answered
    /// `{"accepted":true,"queued":true}` had nowhere to show up afterwards, so
    /// a user who sent one while a turn was running saw a session that looked
    /// finished and a message that had apparently vanished.
    ///
    /// Counts the steered messages too: they have been handed to claude but
    /// have not run, so a session with one of them outstanding is not finished.
    /// Always equal to `queuedMessages.len()`.
    pub queued: u32,
    /// The messages behind that count, in the order they will run.
    ///
    /// Steered messages first — they are already in claude's stdin, so they run
    /// before anything still waiting here — then the queue proper, oldest
    /// first.
    pub queued_messages: Vec<QueuedMessage>,
    /// How many background tasks claude is still minding.
    ///
    /// These outlive the turn that started them, which is why a session can be
    /// `idle` and still be doing work.
    ///
    /// Stays the authoritative number. `tasks` is capped and this is not, so a
    /// session minding more than the cap reports all of them here and describes
    /// the first hundred there.
    pub background: u32,
    /// Those tasks, oldest first, as far as the CLI described them.
    ///
    /// Empty whenever `background` is zero, and never longer than the cap the
    /// driver publishes — see [`crate::inject::resident::TASKS_ON_THE_WIRE`].
    pub tasks: Vec<BackgroundTask>,
    /// Whether the head of the queue is parked waiting for that background work.
    ///
    /// A message asking for a different model, effort or account needs a new
    /// process, and replacing the running one would kill whatever it is minding
    /// in the background — so it waits. Without this the session reads as
    /// simply idle with something queued, which is indistinguishable from a bug.
    pub awaiting_background: bool,
    /// Whether the turn in flight has produced nothing for a long time.
    ///
    /// Never a reason to kill anything; it is the flag a Stop button is offered
    /// beside.
    pub stalled: bool,
    /// When the turn in flight began, by this server's clock, ISO-8601; `null`
    /// between turns.
    ///
    /// The moment the resident left `idle` for `starting` or `working`, so a
    /// list row can say how long a session has been at it. Kept across a
    /// recovery inside one turn, and cleared when the turn ends. Null for a
    /// driver that does not track it (codex's app-server).
    pub turn_started_at: Nullable<String>,
    /// Whether claude is blocked on an `AskUserQuestion` this server is holding
    /// open, mid-turn, waiting for the user's answer.
    ///
    /// The process' own word, so it is what [`apply_driver`] reports as the
    /// row's `asking` — while the turn is `working`, which is exactly when a
    /// question is open. Always false for codex. `#[serde(default)]` for rows a
    /// cache wrote before the field existed.
    #[serde(default)]
    pub asking: bool,
}

impl ResidentInfo {
    /// This resident's contribution to the row's [`SessionActivity`].
    ///
    /// Order matters, and it is the order a person reads the session in: a turn
    /// in flight is the headline whatever else is true, a queue is the next
    /// thing worth saying, and background work is what is left when nothing
    /// else is happening.
    pub fn activity(&self) -> SessionActivity {
        match self.phase {
            ResidentPhase::Starting | ResidentPhase::Working => SessionActivity::Working,
            ResidentPhase::Idle if self.queued > 0 => SessionActivity::Queued,
            ResidentPhase::Idle if self.background > 0 => SessionActivity::Background,
            ResidentPhase::Idle => SessionActivity::Idle,
        }
    }
}

/// What the driver subsystem knows about the sessions being listed.
///
/// A snapshot, taken once per listing and read per row, in the same shape and
/// for the same reason as the account bindings [`list_and_filter`] is handed:
/// the session module describes rows and must not reach into the subsystem that
/// drives them, and a per-row lock of the injection registry would be a lock
/// per row.
#[derive(Debug, Default, Clone)]
pub struct DriverStates {
    /// Whose snapshot this is. Every map below is keyed by
    /// [`SessionRef::scoped_key`], so reading one back needs the server that
    /// wrote it — which, for a snapshot taken from this process' own registry,
    /// is always the local one.
    server: ServerId,
    /// [`SessionRef::scoped_key`] -> resident state.
    ///
    /// Both sources are keyed the same way. They were not: claude residents
    /// were filed under the bare session id and codex ones under `codex/{id}`,
    /// an asymmetry nothing outside this snapshot could see and nothing inside
    /// it relied on.
    residents: HashMap<String, ResidentInfo>,
    /// [`SessionRef::scoped_key`] of every run this server has in flight,
    /// resident or not. This also covers legacy Codex exec processes adopted
    /// during an upgrade.
    running: HashSet<String>,
    /// [`SessionRef::scoped_key`] -> finish time of the last run this server
    /// saw end.
    ///
    /// Codex normally writes `turn_aborted` before an interrupted process exits,
    /// but a hard stop can leave its rollout ending in `task_started`. The
    /// transcript then guesses `running` forever (or at least until its
    /// fifteen-minute freshness window expires) even though the driver just
    /// reaped the process. The finish time lets the driver correct that stale
    /// tail without masking a newer Codex process that appended afterwards.
    finished: HashMap<String, String>,
}

impl DriverStates {
    /// Build a snapshot from what the injection registry answered.
    pub fn new(server: ServerId, residents: HashMap<String, ResidentInfo>, running: HashSet<String>, finished: HashMap<String, String>) -> Self {
        Self {
            server,
            residents,
            running,
            finished,
        }
    }

    /// The key every map here is filed under — the registry's own.
    fn key(&self, source: Source, id: &str) -> String {
        SessionRef::new(self.server.clone(), source, id).scoped_key()
    }

    /// This server's resident for a session, if it has one.
    pub fn resident(&self, source: Source, id: &str) -> Option<ResidentInfo> {
        self.residents.get(&self.key(source, id)).cloned()
    }

    /// Whether this server has a run in flight for a session.
    fn injecting(&self, source: Source, id: &str) -> bool {
        self.running.contains(&self.key(source, id))
    }

    /// When the last run this server knew about finished, if it has finished.
    fn finished_at(&self, source: Source, id: &str) -> Option<&str> {
        self.finished.get(&self.key(source, id)).map(String::as_str)
    }

    /// Fold the driver's account into every row.
    ///
    /// Called from [`list_and_filter`] *before* [`crate::attention::apply`],
    /// because `unread` is computed from `status` and the corrected status is
    /// the one it must be computed from: a session whose resident is mid-turn
    /// is not waiting on the user, whatever the tail of its transcript looks
    /// like.
    pub fn apply(&self, sessions: &mut [SessionSummary]) {
        for summary in sessions {
            let resident = self.resident(summary.source, &summary.id);
            let injecting = self.injecting(summary.source, &summary.id);
            let finished_at = self.finished_at(summary.source, &summary.id);
            apply_driver(summary, resident.as_ref(), injecting, finished_at);
        }
    }
}

/// Correct one row with what the driver knows, where it knows anything.
///
/// The rule for the contradiction this exists to settle: **the driver wins.**
/// `status` is a heuristic over a file; a resident is the process writing that
/// file, and it knows whether a turn is in flight without having to guess. So a
/// session whose resident is working, starting, or holding a queue is reported
/// as `running` however the transcript reads — which is what stops a phone
/// showing "waiting for your input" at a session that is thirty seconds into a
/// half-hour turn.
///
/// It normally corrects *upwards*. A resident that is genuinely idle leaves
/// `status` alone, because the transcript is then the better witness: it can
/// tell "waiting on you" from "nothing to say", and the driver cannot. The one
/// downward correction is a Codex run the driver reaped after the rollout's
/// last write: no process remains, so a `task_started` tail is stale. A rollout
/// written after that finish wins again, preserving the guard against a Codex
/// process started somewhere else.
pub fn apply_driver(summary: &mut SessionSummary, resident: Option<&ResidentInfo>, injecting: bool, finished_at: Option<&str>) {
    let codex_run_ended_after_last_write =
        summary.source == Source::Codex && finished_at.is_some_and(|finished_at| summary.updated_at.as_str() < finished_at);
    summary.activity = match resident {
        Some(resident) => resident.activity(),
        None if injecting => SessionActivity::Working,
        None if codex_run_ended_after_last_write => SessionActivity::Idle,
        None => SessionActivity::from_status(summary.status),
    };
    if summary.activity == SessionActivity::Working || summary.activity == SessionActivity::Queued {
        summary.status = SessionStatus::Running;
        // Read off the transcript, a question is not holding up a turn that is
        // running, or one about to run the message that answers it. A resident
        // overrides this just below with the one account that is not a guess.
        summary.asking = false;
    } else if codex_run_ended_after_last_write && summary.status == SessionStatus::Running {
        // The closest transcript-native equivalent is `turn_aborted`, which
        // Codex's own parser classifies as Waiting. It also keeps partial output
        // eligible for attention instead of pretending the aborted turn wrote
        // nothing at all.
        summary.status = SessionStatus::Waiting;
    }
    // A resident's own word on whether it is blocked on the user, and the only
    // one there is for a session this server drives: claude asks *mid-turn*
    // and the turn stays in flight until the answer comes, so a rule that read
    // "working" as "not asking" would hide every question it holds. Codex
    // never asks, and its resident says so.
    if let Some(resident) = resident {
        summary.asking = resident.asking;
    }
    summary.resident = Nullable(resident.cloned());
}

/// Whose session this is, from this server's point of view.
///
/// The distinction the whole list is built on. A server *owns* the sessions it
/// made: a claude transcript in the Claude root this instance owns, or a codex
/// thread whose id is in this instance's own record. Everything else — the
/// user's own `~/.claude`, another remote-hand's sessions, a codex thread started
/// from a terminal — is **external**: readable, searchable, openable, but not
/// this server's to drive.
///
/// Ownership is not a permission check; it is a statement about who is allowed to
/// *resume* a conversation. Two servers resuming one transcript interleave their
/// turns into it, which is the failure this replaces with a `409`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum SessionScope {
    /// This server made it, and may drive it.
    Owned,
    /// Somebody else made it. Browse-only from this server.
    External,
}

/// Which scopes a listing asks for.
///
/// Separate from [`SessionScope`] because it has a third answer a row never has:
/// `all`, which is what the global feed and the directory picker want.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum ScopeFilter {
    /// Only this server's own sessions. The default, and the sidebar's view.
    #[default]
    Owned,
    /// Only sessions somebody else made.
    External,
    /// Both, each row saying which it is.
    All,
}

impl ScopeFilter {
    /// Whether a row of this scope belongs in the listing.
    pub fn admits(self, scope: SessionScope) -> bool {
        match self {
            ScopeFilter::All => true,
            ScopeFilter::Owned => scope == SessionScope::Owned,
            ScopeFilter::External => scope == SessionScope::External,
        }
    }
}

/// One row in the session list.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    /// Pinned above the ordinary sidebar list in this server instance.
    #[serde(default)]
    pub pinned: bool,
    /// The CLI's session id.
    pub id: String,
    /// Which CLI it belongs to.
    pub source: Source,
    /// Whether this server made the session, or is only looking at it.
    pub scope: SessionScope,
    /// Which remote-hand server this row came from, as [`ServerId`] spells it.
    ///
    /// It answers two subtly different questions depending on `scope`, and the
    /// difference is worth stating because a client will eventually route on
    /// this:
    ///
    /// * **`owned`** — the server that *made* the session. Read from the
    ///   session's own ownership row ([`owned_sessions`]), so it survives a
    ///   restart and is a claim about history.
    /// * **`external`** — the server the session is *reachable through*. A
    ///   discovered transcript (the user's own `~/.claude`, a codex thread
    ///   started from a terminal) was made by nobody here; what is true of it
    ///   is that this server can see the file and serve its contents, so this
    ///   server names itself.
    ///
    /// Both answers are "the server to ask about this row", which is what makes
    /// one field reasonable; neither is "the machine the work happened on", and
    /// nothing should read it as that.
    ///
    /// Filled in by the ownership overlay — `claude::mark_owned` and
    /// `codex::apply_ownership` — alongside `scope`, because both can change
    /// without the transcript being touched and neither may be cached against
    /// it. `#[serde(default)]` for the summaries [`crate::db::index`] cached
    /// before this field existed: they parse, and the overlay fills them in.
    #[serde(default)]
    pub server: String,
    /// Best available title: user override, CLI custom title, summary, or first message.
    pub title: String,
    /// Working directory the session ran in.
    pub cwd: Nullable<String>,
    /// The git repository `cwd` belongs to, as a canonical path, or `null`.
    ///
    /// Every worktree of one repository shares this, so a client can group the
    /// four checkouts of one project into one heading instead of four. Purely a
    /// display key: it never widens what a server lists, and two sessions sharing
    /// a `repoKey` have no relationship beyond the repository they sit in.
    pub repo_key: Nullable<String>,
    /// Git branch recorded at session start.
    pub git_branch: Nullable<String>,
    /// Transcript mtime, ISO-8601.
    pub updated_at: String,
    /// When the session was started, ISO-8601.
    ///
    /// The first timestamp its own transcript recorded, falling back to the
    /// file's mtime for a transcript too odd to date. Appending to a session
    /// moves `updatedAt` and never this, which is what lets recent directories
    /// be ranked by where work was started rather than by what is loudest.
    pub created_at: String,
    /// Transcript size in bytes.
    pub size_bytes: u64,
    /// Absolute path of the transcript file.
    pub file_path: String,
    /// Whether a live CLI process is attached, and what it is doing.
    ///
    /// Derived from a live-process registration plus the tail of the
    /// transcript, then corrected by [`apply_driver`] wherever this server
    /// drives the session itself. Prefer [`SessionSummary::activity`], which
    /// says the same thing with more resolution and never has to guess.
    pub status: SessionStatus,
    /// Whether anything is in flight, and if not, what is holding it up.
    ///
    /// The field a list row should render "busy" from. Unlike `status` it can
    /// tell a queued message from an idle session, and background work from
    /// nothing at all.
    pub activity: SessionActivity,
    /// This server's resident process for the session, when it has one.
    ///
    /// Null for every session this server does not drive, which is most of
    /// them: an external transcript, a codex thread, a claude somebody's own
    /// terminal has open.
    ///
    /// [`Nullable`] rather than a bare `Option` because the field is always
    /// *present* on the wire and is `null` when there is no resident, which is
    /// what a client reading it as `row.resident?.phase` needs the schema to
    /// say.
    pub resident: Nullable<ResidentInfo>,
    /// Which Claude config root holds the session file (claude sessions only).
    ///
    /// Added with the accounts subsystem (`crate::claude_roots`); `None` for
    /// codex, which has one root.
    pub claude_root: Option<ClaudeRoot>,
    /// Account the session is pinned to, for managed-root sessions.
    pub account_id: Option<String>,
    /// When the assistant last spoke on the main chain, ISO-8601.
    ///
    /// Derived from the transcript by [`crate::attention`], not stored; `null`
    /// for a session the assistant has not said anything in yet.
    pub attention_at: Nullable<String>,
    /// Whether the session is waiting on the user: the assistant has spoken
    /// since the user last looked, and the session is not mid-turn.
    pub unread: bool,
    /// How many agent messages arrived since the user last looked, at most 99.
    ///
    /// Assistant text only — not tool calls or results, not thinking, not the
    /// CLI's own synthetic notices — counted from the start for a session never
    /// opened. Not suppressed while a turn runs, unlike `unread`: a working
    /// session can be holding answers nobody has read. `unread` is never true
    /// with a count of zero. See [`crate::attention::unread`].
    pub unread_count: u32,
    /// Whether the agent has stopped to ask the user something, and cannot go
    /// on without an answer.
    ///
    /// Much narrower than `status: waiting`, which only says a live process
    /// finished its turn — and this server keeps a finished session's process
    /// alive, so that is true of nearly every session it drives. For a session
    /// this server drives it is the resident's own word ([`ResidentInfo::asking`]):
    /// true from the moment claude's `AskUserQuestion` arrives until it is
    /// answered, dismissed or withdrawn — mid-turn, so while `activity` says
    /// `working`. For any other live claude it is read off the transcript: a
    /// tail ending on an `AskUserQuestion` no result has answered yet, and never
    /// while [`apply_driver`] knows a turn is in flight. Always false for codex:
    /// it runs with approvals off, and every interactive request its app server
    /// makes is refused, so it has no way to stop and ask.
    ///
    /// `#[serde(default)]` for the summaries [`crate::db::index`] cached before
    /// this field existed; the live overlay fills it in.
    #[serde(default)]
    pub asking: bool,
    /// The subagents this session delegated work to, as a tree: each child
    /// with its own children, when the agent runtime records who spawned whom.
    ///
    /// An overlay like `unread` ([`subagents::apply`]), never cached with the
    /// row: a child writes to a file of its own, so the parent's transcript can
    /// sit still while its children change. Empty for a session with none, and
    /// for a runtime that keeps no parent links. Forks are not children — only
    /// work a session delegated is.
    #[serde(default)]
    pub children: Vec<subagents::SessionChild>,
    /// The session this one is a delegated subagent of, for a thread the
    /// runtime keeps as a session of its own (Codex), when it is opened by id:
    /// the way back to the conversation that spawned it. `null` otherwise.
    #[serde(default)]
    pub parent_id: Nullable<String>,
}

/// The session list response.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct SessionList {
    /// The page of sessions, newest first.
    pub sessions: Vec<SessionSummary>,
    /// How many sessions matched before the limit was applied.
    pub total: usize,
}

/// State of a message remote-hand injected into a session.
///
/// Filled in by `crate::inject::InjectionRegistry`; `null` for a session
/// remote-hand has never driven.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct Injection {
    /// Where the run got to.
    pub state: InjectionRunState,
    /// Failure text, when it failed.
    pub error: Nullable<String>,
    /// Whether the user interrupted it.
    pub interrupted: bool,
    /// When it started, ISO-8601.
    pub started_at: String,
    /// When it finished, ISO-8601.
    pub finished_at: Nullable<String>,
}

/// How an injected run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum InjectionRunState {
    /// Still going.
    Running,
    /// Finished cleanly.
    Ok,
    /// Finished with an error.
    Failed,
}

/// The model/effort a session actually ran with, per its own transcript.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct CliChoice {
    /// Model id.
    pub model: Nullable<String>,
    /// Reasoning effort.
    pub effort: Nullable<String>,
    /// Timestamp of the transcript entry the reading came from.
    pub at: Nullable<String>,
}

impl CliChoice {
    /// The newest model/effort pair a transcript's tail names.
    ///
    /// Both CLIs record the pair once per turn — Claude on every assistant
    /// entry, Codex in the `turn_context` that opens one — so the newest entry
    /// that names either is what the session is set to run. `extract` reads one
    /// entry and returns its pair, or `None` when the entry says nothing about
    /// them.
    pub async fn read_tail(
        path: &Path,
        tail_bytes: u64,
        extract: impl Fn(&serde_json::Map<String, serde_json::Value>) -> Option<(Option<String>, Option<String>)>,
    ) -> Option<Self> {
        let lines = util::read_tail_lines(path, tail_bytes).await.ok()?;
        for line in lines.iter().rev() {
            let Some(entry) = util::try_parse(line) else { continue };
            let Some((model, effort)) = extract(&entry) else { continue };
            return Some(Self {
                model: Nullable(model),
                effort: Nullable(effort),
                at: Nullable(entry.get("timestamp").and_then(serde_json::Value::as_str).map(str::to_owned)),
            });
        }
        None
    }
}

/// Session state plus a page of its transcript.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    /// The list row for this session.
    pub summary: SessionSummary,
    /// A remote-hand-injected run, when one is in flight.
    pub injection: Nullable<Injection>,
    /// Whether the composer may send right now.
    pub can_send: bool,
    /// Why it may not.
    pub send_block_reason: Nullable<String>,
    /// Model/effort the session last actually ran with.
    pub cli_choice: Nullable<CliChoice>,
    /// How much of the context window the last request occupied; `null` until
    /// the session has made one.
    pub context_tokens: Option<u64>,
    /// The window those tokens are measured against — the size the CLI reported
    /// itself where it does, and otherwise inferred from the model id.
    pub context_window: u64,
    /// Config root of a claude session; `null` for codex.
    pub claude_root: Option<ClaudeRoot>,
    /// Account a managed claude session runs under; `null` when it runs locally.
    pub account: Option<AccountRef>,
    /// The latest time this session was moved off an account that hit a limit,
    /// or tried to be; absent or `null` when it never was.
    pub rotation: Option<crate::accounts::rotation::SessionRotation>,
    /// What became of the messages clients sent this session under a
    /// `clientMessageId`, over the last two hours (at most 100, oldest first).
    pub receipts: Vec<crate::inject::receipts::MessageReceipt>,
    /// The questions the agent stopped to ask (`AskUserQuestion`), oldest
    /// first: the one it is blocked on, if any, and the recent ones with what
    /// became of them. At most 20, from the last week; empty for codex.
    pub questions: Vec<crate::inject::questions::SessionQuestion>,
    /// Opaque transcript signature; echo it back via `?sig=` to poll cheaply.
    pub sig: String,
    /// False when `sig` matched: `items` is empty and client state stays as-is.
    pub items_changed: bool,
    /// True when `items` is a full snapshot; false when it appends after `?after=`.
    pub reset: bool,
    /// The page of transcript items.
    pub items: Vec<TranscriptItem>,
    /// Absolute index of the first item in `items`.
    pub start_index: usize,
    /// Absolute index one past the last item in `items`.
    pub end_index: usize,
    /// How many items the transcript holds in total.
    pub total_items: usize,
    /// Whether older items exist before `start_index`.
    pub has_more_before: bool,
    /// How many items were dropped off the front of this page.
    pub omitted_items: usize,
}

/// Server and data-source information.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Which instance this server owns; `default` unless `APP_INSTANCE` says
    /// otherwise. Two servers on one machine must differ here, and everything
    /// that survives a restart is scoped to it.
    pub instance: String,
    /// The name of the server answering, as [`ServerId`] spells it.
    ///
    /// The same value as [`Self::instance`] today, and additive on purpose: the
    /// two are one concept seen from two sides — `instance` is where this
    /// server's private directory is, `serverId` is what a session reference
    /// made on it is scoped to. A client that wants to say *which* server a
    /// session came from reads this one; `instance` keeps meaning what it has
    /// always meant to the clients already reading it.
    pub server_id: String,
    /// The machine's host name.
    pub hostname: String,
    /// Node-style platform name (`darwin`, `linux`, `win32`).
    pub platform: String,
    /// Runtime version. The Rust server reports its own runtime here.
    pub node_version: String,
    /// Exactly which build this is: the crate version, plus the commit it was
    /// built from. A client that watches this can tell that the server it is
    /// talking to has been replaced under it — see [`crate::selfupdate`].
    pub server_version: String,
    /// The home directory sessions are discovered under.
    pub home_dir: String,
    /// Where Claude Code keeps its transcripts.
    pub claude_dir: String,
    /// Where *this instance* keeps the transcripts of the sessions it owns.
    ///
    /// Per-instance since ownership landed: two servers on one machine report
    /// two different directories here, and each lists only what is in its own.
    pub claude_managed_dir: String,
    /// Where Codex keeps its transcripts.
    pub codex_dir: String,
    /// How many Claude sessions were found.
    pub claude_session_count: usize,
    /// How many Codex sessions were found.
    pub codex_session_count: usize,
    /// When this server process started, ISO-8601.
    pub server_started_at: String,
    /// Every plugin installed in this instance, as the instance sees it — the
    /// same list `GET /api/plugins` answers. Empty when none is installed, and
    /// empty for a request bridged from droppen.app.
    pub plugins: Vec<crate::plugins::wire::PluginStatus>,
}

/// Every directory session discovery reads, rooted at one home directory.
///
/// Threading this through instead of reading `homedir()` at module scope (as the
/// TypeScript does) is what lets the integration test point the server at a
/// synthetic `~/.claude` tree.
///
/// Every directory the environment may move is resolved once, at construction,
/// through [`claude_roots::env_dir`] — the one reader of those overrides.
/// `DROPPEN_CONFIG_DIR` and `DROPPEN_DATA_DIR` name remote-hand's own roots;
/// `DROPPEN_CLAUDE_LOCAL_ROOT` and `DROPPEN_CLAUDE_MANAGED_ROOT` name the two
/// Claude ones (each still read under its old `REMOTE_HAND_*` name,
/// [`crate::env`]). Resolving them
/// here rather than on each access is what makes a `Paths` a value: two of them
/// can disagree, and neither changes under a handler because something else
/// touched the process environment.
#[derive(Debug, Clone)]
pub struct Paths {
    /// The home directory everything hangs off.
    pub home: PathBuf,
    config_dir: PathBuf,
    data_dir: PathBuf,
    claude_local: PathBuf,
    /// Which instance's directory the owned half of the world lives in.
    instance: String,
    claude_managed: PathBuf,
}

impl Paths {
    /// Root a path set at `home`, for the default instance.
    ///
    /// A server always calls [`Paths::for_instance`] instead; this is the shape
    /// every unit test and every path-only caller wants, and it names the same
    /// instance a server started with no `APP_INSTANCE` would.
    pub fn new(home: PathBuf) -> Self {
        Self::for_instance(home, crate::instance::DEFAULT_INSTANCE)
    }

    /// Root a path set at `home`, for the instance this server owns.
    ///
    /// The instance name is part of a `Paths` rather than looked up per access
    /// for the same reason the environment overrides are resolved once: two of
    /// these coexist in one test process, and neither may change under a handler
    /// because something else moved.
    pub fn for_instance(home: PathBuf, instance: &str) -> Self {
        let config_dir = claude_roots::env_dir("DROPPEN_CONFIG_DIR").unwrap_or_else(|| home.join(".config").join("remote-hand"));
        let data_dir = claude_roots::env_dir("DROPPEN_DATA_DIR").unwrap_or_else(|| home.join(".local").join("share").join("remote-hand"));
        let claude_local = claude_roots::env_dir(claude_roots::LOCAL_ROOT_ENV).unwrap_or_else(|| home.join(".claude"));
        // The managed root defaults *inside this instance's own directory*, so
        // it is resolved after the data directory and inherits any
        // `DROPPEN_DATA_DIR` override. Two servers on one machine therefore
        // never see each other's sessions — which is the whole of the ownership
        // model this file's `Managed` root now encodes.
        let claude_managed = claude_roots::env_dir(claude_roots::MANAGED_ROOT_ENV)
            .unwrap_or_else(|| crate::instance::instance_root(&data_dir, instance).join("claude"));
        Self {
            home,
            config_dir,
            data_dir,
            claude_local,
            instance: instance.to_string(),
            claude_managed,
        }
    }

    /// The instance whose directory the owned half of this path set lives in.
    pub fn instance(&self) -> &str {
        &self.instance
    }

    /// `<data>/instances/<name>` — this instance's private directory.
    pub fn instance_dir(&self) -> PathBuf {
        crate::instance::instance_root(&self.data_dir, &self.instance)
    }

    /// The machine-wide state database: everything in the config directory that
    /// is not a file a person edits.
    ///
    /// Machine-wide because the things in it are: a Claude account is a login
    /// somebody registered, a title is a name someone gave a session, and a
    /// session marked read is read on every server on this machine. It is also
    /// the scope where the current files are least defensible — two servers
    /// share this directory right now and guard it with mutexes private to each
    /// process, so the loser of a race simply loses their edit. See
    /// [`crate::db`].
    ///
    /// One table in it is no longer written: `users`, the *local* accounts,
    /// which moved to [`Paths::instance_db`] when it turned out that a machine
    /// runs one instance per person as readily as one per purpose. What is
    /// there is frozen rather than dropped, so a rolled-back binary still finds
    /// accounts and stays shut; [`crate::auth::users`] has the argument.
    pub fn config_db(&self) -> PathBuf {
        self.config_dir.join("state.db")
    }

    /// This instance's own state database: the runs, the apps, the layouts, the
    /// accounts and the logins they mint, the schedule.
    ///
    /// Separate from [`Paths::config_db`] because the instance boundary is a
    /// real one — stopping the staging server must not log the phone out of the
    /// default one — and separate files are the cheapest way to keep a boundary
    /// that has already been got wrong twice (`screens.json` and `push.json`
    /// were machine-wide and clobbered each other).
    pub fn instance_db(&self) -> PathBuf {
        self.instance_dir().join("state.db")
    }

    /// The index over the transcripts, which belongs to nobody.
    ///
    /// A third file rather than a table in [`Paths::instance_db`], because it is
    /// a *cache* and the difference has consequences: it can be deleted at any
    /// time and rebuilt from the corpus, it is the one store here that will
    /// reach hundreds of megabytes, and it does not need the `synchronous =
    /// FULL` the authoritative state does — losing the last few commits to a
    /// power cut costs a re-scan, not a fact. Keeping it out of `state.db` is
    /// what lets all three of those be true at once.
    pub fn index_db(&self) -> PathBuf {
        self.instance_dir().join("index.db")
    }

    /// The sessions this instance created whose transcripts live elsewhere —
    /// every codex thread, and every claude session on the local login. See
    /// [`owned_sessions`].
    pub fn owned_sessions_file(&self) -> PathBuf {
        self.instance_dir().join("owned-sessions.json")
    }

    /// `<data>/claude` — the managed root as it was before it became per-instance.
    ///
    /// Only [`Paths::migrate_legacy_claude_root`] reads this: it is where
    /// every account-backed session of every earlier build lives, and the default
    /// instance claims it once at boot.
    pub fn legacy_claude_managed(&self) -> PathBuf {
        self.data_dir.join("claude")
    }

    /// The directory of one Claude config root.
    pub fn claude_root(&self, root: ClaudeRoot) -> PathBuf {
        match root {
            ClaudeRoot::Local => self.claude_local.clone(),
            ClaudeRoot::Managed => self.claude_managed.clone(),
        }
    }

    /// `~/.claude/projects` — one directory per project, each holding `<uuid>.jsonl`.
    ///
    /// The *local* root only; `crate::claude_roots` owns the managed one and the
    /// fan-out over both.
    pub fn claude_projects(&self) -> PathBuf {
        claude_roots::projects_dir(self, ClaudeRoot::Local)
    }

    /// The projects directory of the root remote-hand owns for account sessions.
    pub fn claude_managed_projects(&self) -> PathBuf {
        claude_roots::projects_dir(self, ClaudeRoot::Managed)
    }

    /// Every projects directory a Claude session can live in, local first.
    pub fn claude_projects_dirs(&self) -> [(ClaudeRoot, PathBuf); 2] {
        ClaudeRoot::ALL.map(|root| (root, claude_roots::projects_dir(self, root)))
    }

    /// `~/.codex`.
    pub fn codex_dir(&self) -> PathBuf {
        self.home.join(".codex")
    }

    /// `~/.codex/sessions`.
    pub fn codex_sessions(&self) -> PathBuf {
        self.codex_dir().join("sessions")
    }

    /// Codex's thread index database.
    pub fn codex_state_db(&self) -> PathBuf {
        self.codex_dir().join("state_5.sqlite")
    }

    /// Codex's optional per-thread name index.
    pub fn codex_session_index(&self) -> PathBuf {
        self.codex_dir().join("session_index.jsonl")
    }

    /// `<data>/codex-claims` — which instance made which codex thread.
    ///
    /// Shared, like `inject-locks` and unlike everything under
    /// [`Paths::instance_dir`]: a claim exists precisely to be read by the
    /// instances that did *not* write it, which is the only way one of them can
    /// tell another's thread from the user's own in the `~/.codex` they share.
    /// See [`codex_claims`].
    pub fn codex_claims_dir(&self) -> PathBuf {
        self.data_dir.join("codex-claims")
    }

    /// Codex's cache of the models its backend offers.
    pub fn codex_models_cache(&self) -> PathBuf {
        self.codex_dir().join("models_cache.json")
    }

    /// remote-hand's own sidecar state, overridable with `DROPPEN_CONFIG_DIR`.
    pub fn config_dir(&self) -> PathBuf {
        self.config_dir.clone()
    }

    /// remote-hand's own data (accounts, uploads, chats, the managed Claude
    /// root), overridable with `DROPPEN_DATA_DIR`.
    pub fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    /// Where composer attachments land, one uuid directory per upload; the CLIs
    /// are pointed at it with `--add-dir`.
    pub fn uploads_dir(&self) -> PathBuf {
        self.data_dir.join("uploads")
    }

    /// Claude has no native archive, so remote-hand keeps its own hidden list.
    pub fn archived_file(&self) -> PathBuf {
        self.config_dir().join("archived.json")
    }

    /// User-set session titles, kept in a sidecar so the CLIs' files stay untouched.
    pub fn titles_file(&self) -> PathBuf {
        self.config_dir().join("titles.json")
    }

    /// UI preferences.
    pub fn prefs_file(&self) -> PathBuf {
        self.config_dir().join("prefs.json")
    }

    /// `<data>/instances/<name>/scheduled-tasks.json` — the cron tasks this
    /// server runs.
    ///
    /// Instance-scoped, like the sidecars around it, and for a sharper reason
    /// than theirs. A layout shared by two instances is confusing; a *task list*
    /// shared by two instances is acted on twice — every server on the machine
    /// would read the same schedule and every one of them would start its own
    /// session for it. See [`crate::schedule`].
    ///
    /// Written `0600`: it holds prompts the user wrote and paths into their
    /// source tree.
    pub fn scheduled_tasks_file(&self) -> PathBuf {
        self.instance_dir().join("scheduled-tasks.json")
    }

    /// When the user last looked at each session — the `read_at` half of
    /// [`crate::attention`].
    pub fn seen_file(&self) -> PathBuf {
        self.config_dir().join("seen.json")
    }

    /// The optional per-model price table that overrides the built-in rates.
    pub fn price_override(&self) -> PathBuf {
        self.home.join(".remote-hand").join("model-prices.json")
    }
}

/// Sessions whose `cwd` is the API-chat scratch directory are hidden from the list.
///
/// `CHATS_CWD` in the former TypeScript server's `chats.ts` — under the *data*
/// directory, not the config one: an instant chat's scratch tree is working
/// state, not settings.
pub fn chats_cwd(paths: &Paths) -> PathBuf {
    paths.data_dir().join("chats")
}

/// Every session both CLIs know about, filtered and newest first.
///
/// The one place the session list is assembled. `GET /api/sessions` and the
/// global feed's snapshot are two views of the same list, and the delta the feed
/// pushes is only meaningful if the row the client already holds came out of the
/// same discovery, the same hiding rule and the same order — so they share this
/// rather than each spelling it out.
///
/// `bindings` is the session-to-account map, which only the caller can read
/// (it lives in the accounts store, and this module sits below it).
///
/// `scope` decides *whose* sessions are listed, and is applied after discovery
/// rather than during it: a session that exists in both Claude roots is one row,
/// owned, and must not reappear as an external one because the external half of
/// the pair is still on disk.
pub async fn list_and_filter(
    paths: &Paths,
    bindings: &std::collections::BTreeMap<String, String>,
    drivers: &DriverStates,
    source: Option<Source>,
    query: Option<&str>,
    scope: ScopeFilter,
) -> Vec<SessionSummary> {
    let (claude_sessions, codex_sessions) = match source {
        Some(Source::Claude) => (claude::list_claude_sessions(paths, bindings).await, vec![]),
        Some(Source::Codex) => (vec![], codex::list_codex_sessions(paths).await),
        None => tokio::join!(claude::list_claude_sessions(paths, bindings), codex::list_codex_sessions(paths)),
    };

    // Instant chats live in their own list, not the session sidebar.
    let hidden = chats_cwd(paths).to_string_lossy().into_owned();
    let mut sessions: Vec<SessionSummary> = claude_sessions
        .into_iter()
        .chain(codex_sessions)
        .filter(|session| session.cwd.as_ref().map(String::as_str) != Some(hidden.as_str()))
        .filter(|session| scope.admits(session.scope))
        .collect();
    // The claude discovery applies the bindings itself, inside the root walk it
    // has to do anyway; codex has no such walk, so its rows are matched up here.
    // Either way the field means the same thing and is filled from the same
    // table — a client reading `accountId` must not have to ask which CLI wrote
    // the row.
    for session in sessions.iter_mut().filter(|session| session.source == Source::Codex) {
        session.account_id = bindings.get(&session.id).cloned();
    }

    if let Some(needle) = query.filter(|needle| !needle.is_empty()) {
        let needle = needle.to_lowercase();
        sessions.retain(|session| {
            session.title.to_lowercase().contains(&needle) || session.cwd.as_ref().is_some_and(|cwd| cwd.to_lowercase().contains(&needle))
        });
    }
    // `localeCompare` on ISO-8601 timestamps is a plain lexicographic compare.
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    // After the truncation-free sort but before the caller pages: `attentionAt`,
    // `unread` and `repoKey` are part of a row, and this is the one place a row
    // is made.
    //
    // The driver goes first, and the order is load-bearing: `apply` corrects
    // `status`, and `unread` is computed *from* `status` — so a session whose
    // resident is mid-turn has to be known to be mid-turn before the badge is
    // decided, or the phone lights up for a session that is busy working.
    drivers.apply(&mut sessions);
    // After the driver: a Claude child counts as working only while its
    // parent is, and that is the driver's answer.
    subagents::apply(paths, &mut sessions).await;
    crate::attention::apply(paths, &mut sessions).await;
    repo::apply(&mut sessions).await;
    pins::apply(paths, &mut sessions);
    sessions.sort_by_key(|session| !session.pinned);
    sessions
}

/// How many recent working directories the new-session picker offers.
pub const RECENT_CWDS_LIMIT: usize = 8;

/// Distinct working directories, the most recently *started in* first.
///
/// Ranked by each directory's newest session **creation**, not its newest
/// activity. The difference is the whole point: one long-running session that is
/// still being appended to would otherwise pin its directory to the top forever,
/// which is the opposite of the question the picker asks — "where have I been
/// starting work lately".
///
/// Paths come back exactly as the session list reports them, absolute and
/// unabbreviated; shortening `$HOME` to `~` is the client's display choice, and
/// doing it here would hand back a path nothing can `cd` into.
///
/// Two things it deliberately does not inherit from the sidebar:
///
/// * **every scope counts.** "Where was I working" is a fact about the machine,
///   not about which server made the session, and a fresh instance whose picker
///   offered nothing would be worse than useless.
/// * **one entry per repository.** Four worktrees of one project are four
///   directories but one place the user thinks of, so they collapse to whichever
///   was started in most recently, keyed by `repoKey`. A directory that is not in
///   a repository is its own key, so nothing outside git is merged with anything.
pub async fn recent_cwds(paths: &Paths, bindings: &std::collections::BTreeMap<String, String>, source: Option<Source>) -> Vec<String> {
    // No driver snapshot: this answers "where was I working", which is a
    // question about creation times and says nothing about what is running.
    let sessions = list_and_filter(paths, bindings, &DriverStates::default(), source, None, ScopeFilter::All).await;
    // repository -> (creation of the newest session in it, the cwd that session ran in)
    let mut newest: std::collections::HashMap<String, (String, String)> = std::collections::HashMap::new();
    for session in sessions {
        let Some(cwd) = session.cwd.as_ref().filter(|cwd| !cwd.is_empty()).cloned() else {
            continue;
        };
        let key = session.repo_key.as_ref().cloned().unwrap_or_else(|| cwd.clone());
        let entry = newest.entry(key).or_default();
        // ISO-8601 compares lexicographically, as everywhere else in this module.
        if session.created_at > entry.0 {
            *entry = (session.created_at, cwd);
        }
    }
    let mut ranked: Vec<(String, String)> = newest.into_values().map(|(created_at, cwd)| (cwd, created_at)).collect();
    // Newest first; ties broken by name so the answer is stable across calls.
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.into_iter().take(RECENT_CWDS_LIMIT).map(|(cwd, _)| cwd).collect()
}

/// The `error` a send to somebody else's session is refused with.
///
/// A machine-readable token rather than a sentence, like `account_required`
/// beside it: the client turns it into a read-only notice, and a wording change
/// must not break that.
pub const EXTERNAL_SESSION_ERROR: &str = "external_session";

/// Why an external session cannot be sent to, in one line for the composer.
pub fn external_send_block_reason(source: Source) -> String {
    match source {
        Source::Claude => "this session was not started by this instance; continue it where it was started".to_string(),
        Source::Codex => "this codex session was started elsewhere; continue it where it was started".to_string(),
    }
}

/// Whether the composer may send to this session, and why not.
///
/// Port of `sendability` in the former TypeScript server's `session-feed.ts`,
/// plus the arms the TypeScript never had. `scope` is the ownership arm: a
/// session this server did not make is browse-only, because resuming it would
/// interleave this server's turns with those of whoever actually owns it.
/// `injecting` is the injection arm the probe left out: a one-shot run
/// remote-hand started blocks the composer regardless of what the CLI's own
/// registration says.
///
/// `resident` is what a session driven by a process rather than a run needs, and
/// it lifts both of the other two block arms:
///
/// * **the injection arm.** A second `claude --print` against one transcript
///   would be two agents writing over each other, which is why an in-flight run
///   used to be a refusal. One process taking messages in order cannot do that,
///   so a message sent mid-turn is queued behind it and the composer stays open.
/// * **the live-process arm.** That arm exists to keep remote-hand out of a
///   conversation the user has open in their own terminal — and a resident
///   registers itself in exactly the same place, because it *is* a real
///   `claude`. Without this exemption the first turn would be the last one this
///   server could send.
///
/// Ownership still outranks everything, including a resident: a run against an
/// external session cannot exist, so reporting one would be a lie about a
/// session this server has no claim on.
pub fn sendability(source: Source, scope: SessionScope, status: SessionStatus, injecting: bool, resident: bool) -> (bool, Option<String>) {
    if scope == SessionScope::External {
        return (false, Some(external_send_block_reason(source)));
    }
    if resident {
        return (true, None);
    }
    if injecting {
        return (false, Some("a message from Droppen is still running".into()));
    }
    match (source, status) {
        (Source::Claude, status) if status != SessionStatus::Idle => (false, Some("attached to a live local Claude Code process".into())),
        (Source::Codex, SessionStatus::Running) => (false, Some("session is currently running".into())),
        _ => (true, None),
    }
}

/// User-set titles, which outrank anything derived from a transcript.
///
/// Keyed `source:id` in one shared sidecar, so both CLIs read them through the
/// same filter.
async fn custom_titles(paths: &Paths, source: Source) -> HashMap<String, String> {
    let prefix = format!("{source}:");
    crate::drive::titles::all(paths)
        .into_iter()
        .filter_map(|(key, value)| {
            let id = key.strip_prefix(&prefix)?;
            Some((id.to_string(), value))
        })
        .collect()
}

/// The mtime+size signature the transcript cache and the `?sig=` poll share.
pub async fn file_signature(path: &Path) -> Option<String> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    Some(format!("{}:{}", metadata.len(), mtime_ms(&metadata)))
}

/// Milliseconds since the epoch, matching Node's `stat().mtimeMs`.
pub fn mtime_ms(metadata: &std::fs::Metadata) -> f64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// An ISO-8601 UTC timestamp, matching `Date.prototype.toISOString()`.
pub fn iso8601(time: std::time::SystemTime) -> String {
    let duration = time.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs() as i64;
    let millis = duration.subsec_millis();

    // Civil-from-days (Howard Hinnant's algorithm), so no date crate is needed.
    let days = secs.div_euclid(86_400);
    let time_of_day = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        time_of_day / 3_600,
        (time_of_day % 3_600) / 60,
        time_of_day % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_matches_javascript() {
        assert_eq!(iso8601(std::time::UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_700_000_000_123);
        assert_eq!(iso8601(t), "2023-11-14T22:13:20.123Z");
    }

    #[test]
    fn a_live_claude_session_cannot_be_sent_to() {
        let owned = SessionScope::Owned;
        assert!(sendability(Source::Claude, owned, SessionStatus::Idle, false, false).0);
        assert!(!sendability(Source::Claude, owned, SessionStatus::Waiting, false, false).0);
        assert!(sendability(Source::Codex, owned, SessionStatus::Waiting, false, false).0);
        assert!(!sendability(Source::Codex, owned, SessionStatus::Running, false, false).0);
    }

    #[test]
    fn an_in_flight_injection_outranks_every_other_verdict() {
        let (can_send, reason) = sendability(Source::Codex, SessionScope::Owned, SessionStatus::Idle, true, false);
        assert!(!can_send);
        assert_eq!(reason.as_deref(), Some("a message from Droppen is still running"));
    }

    /// The two arms a resident lifts, and the one it does not.
    ///
    /// Both of the block arms below would fire for a healthy resident, and each
    /// would be wrong in the same way: the busy CLI they are describing is this
    /// server's own, taking messages in order. If either survived, a session
    /// would answer its first message and refuse every one after it.
    #[test]
    fn a_resident_session_takes_messages_while_it_is_busy() {
        let owned = SessionScope::Owned;
        assert!(
            sendability(Source::Claude, owned, SessionStatus::Idle, true, true).0,
            "a turn in flight is queued behind, not refused"
        );
        assert!(
            sendability(Source::Claude, owned, SessionStatus::Waiting, false, true).0,
            "the live claude the probe found is the resident this server started"
        );
        // Ownership is the one thing it does not lift.
        assert!(!sendability(Source::Claude, SessionScope::External, SessionStatus::Idle, false, true).0);
    }

    /// An idle, unlocked, perfectly healthy session this server did not make is
    /// still not this server's to drive.
    #[test]
    fn somebody_elses_session_is_never_sendable() {
        for source in [Source::Claude, Source::Codex] {
            let (can_send, reason) = sendability(source, SessionScope::External, SessionStatus::Idle, false, false);
            assert!(!can_send, "{source} external sessions are browse-only");
            assert!(reason.is_some_and(|reason| !reason.is_empty()));
        }
        // And ownership is checked before the injection state, which cannot
        // meaningfully exist for a session this server never started.
        assert!(!sendability(Source::Claude, SessionScope::External, SessionStatus::Idle, true, false).0);
    }

    /// A row with nothing driving it, so the transcript heuristic is all there is.
    fn row(status: SessionStatus) -> SessionSummary {
        SessionSummary {
            pinned: false,
            id: "s".into(),
            source: Source::Claude,
            scope: SessionScope::Owned,
            server: "default".into(),
            title: "t".into(),
            cwd: Nullable(None),
            repo_key: Nullable(None),
            git_branch: Nullable(None),
            updated_at: String::new(),
            created_at: String::new(),
            size_bytes: 0,
            file_path: String::new(),
            status,
            activity: SessionActivity::from_status(status),
            resident: Nullable(None),
            claude_root: None,
            account_id: None,
            attention_at: Nullable(None),
            unread: false,
            unread_count: 0,
            asking: false,
            children: Vec::new(),
            parent_id: Nullable(None),
        }
    }

    fn resident(phase: ResidentPhase, queued: u32, background: u32) -> ResidentInfo {
        ResidentInfo {
            phase,
            queued,
            queued_messages: Vec::new(),
            background,
            tasks: Vec::new(),
            awaiting_background: false,
            stalled: false,
            turn_started_at: Nullable(None),
            asking: false,
        }
    }

    /// The contradiction this whole field exists to settle.
    ///
    /// `claude_activity_status` reads the tail of the transcript, and a turn
    /// whose recent writing is all sidechain — an agent thirty seconds into a
    /// half-hour of subagent work — ends on an assistant `text` block and is
    /// read as `waiting`. The driver holds the process and knows better, so it
    /// is the driver that is believed.
    #[test]
    fn a_working_resident_outranks_what_the_transcript_looks_like() {
        let mut summary = row(SessionStatus::Waiting);
        apply_driver(&mut summary, Some(&resident(ResidentPhase::Working, 0, 0)), true, None);
        assert_eq!(summary.activity, SessionActivity::Working);
        assert_eq!(summary.status, SessionStatus::Running, "the driver wins");
        assert_eq!(summary.resident.as_ref().map(|r| r.phase), Some(ResidentPhase::Working));
    }

    /// The launch window, which is exactly when a user is most likely to look.
    #[test]
    fn a_starting_resident_is_already_working() {
        let mut summary = row(SessionStatus::Idle);
        apply_driver(&mut summary, Some(&resident(ResidentPhase::Starting, 1, 0)), false, None);
        assert_eq!(summary.activity, SessionActivity::Working);
        assert_eq!(summary.status, SessionStatus::Running);
    }

    /// The reported bug: a message queued behind a turn had nowhere to show up,
    /// so the session read as finished and the message looked lost.
    #[test]
    fn a_queue_behind_an_idle_process_is_not_an_idle_session() {
        let mut summary = row(SessionStatus::Waiting);
        apply_driver(&mut summary, Some(&resident(ResidentPhase::Idle, 2, 0)), false, None);
        assert_eq!(summary.activity, SessionActivity::Queued);
        assert_eq!(summary.status, SessionStatus::Running, "something is still going to happen");
        assert_eq!(summary.resident.as_ref().map(|r| r.queued), Some(2));
    }

    /// Background work outlives its turn, and is the one kind of work that must
    /// *not* read as a turn in flight: the composer is open, and a message sent
    /// now goes straight in.
    #[test]
    fn background_work_is_its_own_answer() {
        let mut summary = row(SessionStatus::Waiting);
        apply_driver(&mut summary, Some(&resident(ResidentPhase::Idle, 0, 3)), false, None);
        assert_eq!(summary.activity, SessionActivity::Background);
        assert_eq!(summary.status, SessionStatus::Waiting, "corrected upwards only");
    }

    /// A genuinely idle resident leaves the transcript to say what kind of idle
    /// it is: "waiting on you" is a distinction the driver cannot make.
    #[test]
    fn an_idle_resident_does_not_overwrite_the_transcripts_answer() {
        for status in [SessionStatus::Idle, SessionStatus::Waiting] {
            let mut summary = row(status);
            apply_driver(&mut summary, Some(&resident(ResidentPhase::Idle, 0, 0)), false, None);
            assert_eq!(summary.activity, SessionActivity::Idle);
            assert_eq!(summary.status, status);
        }
    }

    /// Without a resident, a question the transcript ends on only asks while
    /// nothing is running: a run in flight is not blocked on the user however
    /// the file reads.
    #[test]
    fn a_question_read_off_a_transcript_only_asks_while_nothing_is_in_flight() {
        for (injecting, asking) in [(true, false), (false, true)] {
            let mut summary = row(SessionStatus::Waiting);
            summary.asking = true;
            apply_driver(&mut summary, None, injecting, None);
            assert_eq!(summary.asking, asking, "{:?} with injecting={injecting}", summary.activity);
        }
    }

    /// With one, the resident decides — and claude asks mid-turn, so the case
    /// that matters is a *working* resident holding a question open. The rule
    /// above would hide exactly that one; a resident that holds nothing open
    /// is not asking, whatever the transcript's tail looks like.
    #[test]
    fn a_resident_holding_a_question_open_is_asking_while_its_turn_is_in_flight() {
        for (phase, queued, background, holding, asking) in [
            (ResidentPhase::Working, 0, 0, true, true),
            (ResidentPhase::Working, 2, 0, true, true),
            (ResidentPhase::Working, 0, 0, false, false),
            (ResidentPhase::Idle, 1, 0, false, false),
            (ResidentPhase::Idle, 0, 0, false, false),
            (ResidentPhase::Idle, 0, 2, false, false),
        ] {
            let mut summary = row(SessionStatus::Waiting);
            summary.asking = true;
            let resident = ResidentInfo {
                asking: holding,
                ..resident(phase, queued, background)
            };
            apply_driver(&mut summary, Some(&resident), phase == ResidentPhase::Working, None);
            assert_eq!(
                summary.asking, asking,
                "{phase:?} queued={queued} background={background} holding={holding}"
            );
        }
        let mut working = row(SessionStatus::Waiting);
        apply_driver(
            &mut working,
            Some(&ResidentInfo {
                asking: true,
                ..resident(ResidentPhase::Working, 0, 0)
            }),
            true,
            None,
        );
        assert_eq!(
            (working.activity, working.status, working.asking),
            (SessionActivity::Working, SessionStatus::Running, true),
            "still working, and asking"
        );
    }

    /// Codex has no residents, so its runs are the only driver signal there is.
    #[test]
    fn a_run_in_flight_counts_as_working_without_a_resident() {
        let mut summary = row(SessionStatus::Idle);
        summary.source = Source::Codex;
        apply_driver(&mut summary, None, true, None);
        assert_eq!(summary.activity, SessionActivity::Working);
        assert_eq!(summary.status, SessionStatus::Running);
        assert!(summary.resident.as_ref().is_none());
    }

    /// A killed Codex process may never append `turn_aborted`, leaving a recent
    /// `task_started` as the transcript's last word. The process the driver just
    /// reaped is stronger evidence than that stale tail.
    #[test]
    fn a_finished_codex_run_outranks_its_stale_running_tail() {
        let mut summary = row(SessionStatus::Running);
        summary.source = Source::Codex;
        summary.updated_at = "2026-09-11T10:00:00.000Z".into();
        apply_driver(&mut summary, None, false, Some("2026-09-11T10:00:01.000Z"));
        assert_eq!(summary.activity, SessionActivity::Idle);
        assert_eq!(summary.status, SessionStatus::Waiting, "the composer is released after the process exits");
    }

    /// The finish record describes only remote-hand's last run. If the rollout
    /// changed later, Codex may now be running in a terminal and the transcript
    /// must regain the deciding vote.
    #[test]
    fn a_newer_codex_tail_outranks_an_old_finish_record() {
        let mut summary = row(SessionStatus::Running);
        summary.source = Source::Codex;
        summary.updated_at = "2026-09-11T10:00:02.000Z".into();
        apply_driver(&mut summary, None, false, Some("2026-09-11T10:00:01.000Z"));
        assert_eq!(summary.activity, SessionActivity::Working);
        assert_eq!(summary.status, SessionStatus::Running, "a newer external process still blocks sending");
    }

    /// A session nobody here drives is described by its transcript alone, and
    /// `waiting` is not work.
    #[test]
    fn an_undriven_row_falls_back_to_its_status() {
        for (status, activity) in [
            (SessionStatus::Running, SessionActivity::Working),
            (SessionStatus::Waiting, SessionActivity::Idle),
            (SessionStatus::Idle, SessionActivity::Idle),
        ] {
            let mut summary = row(status);
            apply_driver(&mut summary, None, false, None);
            assert_eq!(summary.activity, activity, "{status:?}");
            assert_eq!(summary.status, status, "nothing to correct it with");
        }
    }

    /// The snapshot is keyed the way the registry is: every map by
    /// [`SessionRef::scoped_key`], both sources alike.
    #[test]
    fn a_driver_snapshot_is_read_by_the_keys_it_was_built_with() {
        let server = ServerId::new("alice").expect("a valid server name");
        let states = DriverStates::new(
            server.clone(),
            HashMap::from([(
                SessionRef::new(server.clone(), Source::Claude, "s").scoped_key(),
                resident(ResidentPhase::Working, 0, 0),
            )]),
            HashSet::from([SessionRef::new(server.clone(), Source::Codex, "c").scoped_key()]),
            HashMap::new(),
        );
        assert!(states.resident(Source::Claude, "s").is_some());
        assert!(states.resident(Source::Codex, "s").is_none(), "the sources share no key");
        assert!(states.injecting(Source::Codex, "c"));
        assert!(!states.injecting(Source::Claude, "c"));

        // A snapshot another server took is not this one's to read.
        let elsewhere = DriverStates::new(
            ServerId::new("bob").expect("a valid server name"),
            states.residents.clone(),
            states.running.clone(),
            HashMap::new(),
        );
        assert!(elsewhere.resident(Source::Claude, "s").is_none());
        assert!(!elsewhere.injecting(Source::Codex, "c"));

        let mut rows = vec![row(SessionStatus::Waiting)];
        states.apply(&mut rows);
        assert_eq!(rows[0].activity, SessionActivity::Working);
    }

    #[test]
    fn a_scope_filter_admits_exactly_what_it_names() {
        assert!(ScopeFilter::default() == ScopeFilter::Owned, "the sidebar's view is the default");
        assert!(ScopeFilter::Owned.admits(SessionScope::Owned));
        assert!(!ScopeFilter::Owned.admits(SessionScope::External));
        assert!(ScopeFilter::External.admits(SessionScope::External));
        assert!(!ScopeFilter::External.admits(SessionScope::Owned));
        assert!(ScopeFilter::All.admits(SessionScope::Owned) && ScopeFilter::All.admits(SessionScope::External));
    }
}
