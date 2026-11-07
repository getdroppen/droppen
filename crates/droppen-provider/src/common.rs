//! What every CLI's driver shares.
//!
//! Claude Code's resident, Codex's app server and the ACP agents each run a
//! session in the same working directory, name a message or a run with the
//! same kind of id, refuse a session another instance holds in the same words,
//! and report their state and their queue routes' answers in the same types.
//! They live in the base, which every driver may use, because no driver may
//! use another's items.

use std::path::PathBuf;

use droppen_instance::paths::Paths;

use super::questions;
use droppen_sessions::delivery::SessionQuestion;

/// The instant-chat persona: the text the CLI is actually launched with.
pub const CHAT_SYSTEM_PROMPT: &str = concat!(
    "You are a friendly general-purpose assistant in a casual chat app. ",
    "This is not a coding tool: have a normal conversation, answer in the user's language, ",
    "and keep answers conversational. You have no tools; never claim to read files or run commands."
);

/// Where a resident runs, with a fallback that is somewhere and not anywhere.
///
/// A session records the directory it was started in, and that directory can be
/// renamed or deleted between two messages. Refusing to send is the wrong answer
/// — the conversation is still worth continuing — so the fallback is the home
/// directory, the same one a session with no recorded directory at all gets.
///
/// What it must *never* fall back to is "unset", because an unset working
/// directory means the child inherits the server's, and the server's is wherever
/// somebody happened to start it: a developer's checkout, `/`, the directory a
/// supervisor chose. Handing an agent the source tree of the server driving it,
/// silently, because a path went missing, is not a recovery.
///
/// One `stat` on the send path, which is cheap next to the process this is about
/// to start.
pub fn session_workdir(paths: &Paths, cwd: Option<&str>) -> PathBuf {
    match cwd.map(PathBuf::from) {
        Some(dir) if dir.is_dir() => dir,
        _ => paths.home.clone(),
    }
}

/// A fresh random id, for a queued message or a session made here.
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The refusal a claimed session produces, naming the holder when it can.
///
/// The wording of the first clause is the one the in-process map has always
/// produced, deliberately: from the composer's point of view the two are the
/// same condition, and it is the reason the send was refused, not the mechanism
/// that noticed, that belongs in a message to the user.
pub fn busy_message(holder: Option<u32>) -> String {
    match holder {
        Some(pid) => format!("a message is already running for this session (Droppen pid {pid})"),
        None => "a message is already running for this session".to_string(),
    }
}

/// Where a resident is in its cycle.
///
/// "Stalled" is deliberately not one of these: a turn that has gone quiet is
/// still a turn in flight, and making it a state would mean every reader had to
/// know that two of them mean "working". It is a flag beside the phase instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Spawned, but `system:init` has not arrived: nothing may be written yet.
    Starting,
    /// A turn is in flight; its end is a `result` line.
    Working,
    /// Between turns. The process is alive and costs nothing.
    Idle,
}

/// The driver's state, readable without awaiting it.
///
/// A synchronous mirror rather than the truth: the routes and the feed ask "is
/// this session busy" from inside handlers that cannot block on a task, and the
/// answer only has to be good enough to render. Every *decision* is made in the
/// driver, against the real thing.
#[derive(Debug, Clone)]
pub struct ResidentState {
    /// Where the resident is in its cycle.
    pub phase: Phase,
    /// Set when a working turn has been silent past the threshold; cleared by
    /// the next line claude writes. Never a reason to kill anything.
    pub stalled: bool,
    /// How many messages are waiting behind the turn in flight, steered ones
    /// included: a message written to claude's stdin ahead of its turn has not
    /// run yet, so the session is not finished with it.
    pub queued: usize,
    /// Those messages, in the order they will run, as the API reports them.
    ///
    /// Held on the mirror rather than asked of the driver because that is what
    /// the mirror is for: a route that wanted the queue would otherwise have to
    /// await a task which may be inside a thirty-second recovery backoff.
    pub queued_messages: Vec<droppen_sessions::QueuedMessage>,
    /// How many background tasks claude is still minding.
    ///
    /// Deliberately *not* a phase. A turn that dispatched background work ends
    /// like any other — the composer unblocks, the next message goes in — and
    /// the tasks outlive it. What this count changes is only what may end the
    /// *process*: see `Driver::minding_background`.
    pub background: usize,
    /// What those tasks are, oldest first, capped at `TASKS_ON_THE_WIRE`.
    ///
    /// The count above stays the authoritative one: this is the description of
    /// the work, and a row that showed a hundred of anything is already telling
    /// the user something has gone wrong.
    pub tasks: Vec<droppen_sessions::BackgroundTask>,
    /// Set when a settings change is waiting for that background work.
    ///
    /// The message that wants the new settings is at the head of the queue and
    /// has not been sent anywhere. Without this the session would read as
    /// simply idle with something queued, which is indistinguishable from a bug.
    pub awaiting_background: bool,
    /// When the turn in flight began, ISO-8601: set on leaving `Idle`, kept
    /// through a recovery inside the turn, cleared on returning to `Idle`.
    pub turn_started_at: Option<String>,
    /// The `AskUserQuestion` claude is blocked on, by the CLI's request id —
    /// the oldest, should it ever ask two at once. `None` whenever nothing is
    /// waiting on the user.
    ///
    /// On the mirror because two readers cannot wait for the driver: the list
    /// row's "asking you", which is true from the moment the question arrives
    /// and *while the turn is still in flight*, and the send route, which turns
    /// a message typed while a question is open into its answer.
    pub question: Option<String>,
    /// The model the process runs, as its CLI named it (Claude Code's
    /// `system:init`), else as it was asked for; `None` when unknown.
    ///
    /// The feed's record of a long-context ask while the process runs: Claude
    /// Code runs `<model>[1m]` against the 1M window and writes `<model>` in
    /// the transcript, so the process' own name is what says which window a
    /// reading was taken against (`droppen_sessions::context::ContextUsage::run_by`).
    /// Not on the wire.
    pub model: Option<String>,
}

impl ResidentState {
    /// Whether claude is looking after work that outlived its turn.
    pub fn minding_background(&self) -> bool {
        self.background > 0
    }
}

impl From<ResidentState> for droppen_sessions::ResidentInfo {
    fn from(state: ResidentState) -> Self {
        droppen_sessions::ResidentInfo {
            phase: match state.phase {
                Phase::Starting => droppen_sessions::ResidentPhase::Starting,
                Phase::Working => droppen_sessions::ResidentPhase::Working,
                Phase::Idle => droppen_sessions::ResidentPhase::Idle,
            },
            // Saturating rather than `as`: a queue long enough to overflow a
            // `u32` is a bug elsewhere, and reporting it as a small number
            // would hide that bug behind a plausible-looking row.
            queued: state.queued.try_into().unwrap_or(u32::MAX),
            queued_messages: state.queued_messages,
            background: state.background.try_into().unwrap_or(u32::MAX),
            tasks: state.tasks,
            awaiting_background: state.awaiting_background,
            stalled: state.stalled,
            turn_started_at: droppen_http::openapi_ext::Nullable(state.turn_started_at),
            asking: state.question.is_some(),
        }
    }
}

/// What a queue route did, or why it could not.
///
/// Reported rather than logged because every one of these is something the
/// person pressing the button has to be told: the message they meant is already
/// running, or already handed over, or needs a process this one is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueOutcome {
    /// A native protocol refusal, with the message still safely queued.
    Refused {
        /// The CLI's refusal, with credentials redacted before publication.
        reason: String,
    },
    /// The queue changed as asked.
    Done,
    /// This server holds no resident for the session at all.
    NoResident,
    /// No message in the queue has that id.
    ///
    /// The everyday case is a race rather than a typo: the turn in front ended
    /// while the request was in flight and the message is now running.
    Unknown,
    /// The message is already in claude's stdin and cannot be taken back.
    AlreadySteered,
    /// The message asks for settings the running process was not started with,
    /// and the restart it needs cannot be done now: either a turn is in flight,
    /// or the message carries no recipe to start anything from.
    ///
    /// Not the answer to a settings change as such — a steer from *idle* does
    /// the restart, see `Driver::restart_for_steer`.
    NeedsRestart {
        /// What the message wants that the running process does not have, in
        /// the words of `Fingerprint::differences`.
        ///
        /// Carried rather than re-derived at the route, which holds neither
        /// fingerprint: the refusal is only actionable if it says which setting
        /// is in the way.
        differences: String,
    },
    /// The write to claude failed; the message is still queued.
    Unreachable,
}

/// What a stop-one-task request did, or why it could not.
///
/// Three outcomes and no fourth: this server never learns whether the task
/// actually died, because the only report claude makes about that is the next
/// task-list snapshot, which reaches the client by the same channel the list
/// itself came down. So the route answers what it knows — that the request went
/// out — and the panel answers the rest by the row disappearing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopTaskOutcome {
    /// The request is on its way to claude.
    Asked,
    /// This server holds no resident for the session at all.
    NoResident,
    /// The resident is not minding a task by that id.
    NoSuchTask,
}

/// What the user said to a question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// An answer: choices, notes, words of their own — see [`questions::Answer`].
    Answer(questions::Answer),
    /// "Use your judgement": the question is refused, and the agent told so.
    Dismiss,
}

/// What an answer did, or why it could not.
#[derive(Debug, Clone)]
pub enum AnswerOutcome {
    /// Written to claude: its turn goes on with the answer. The question as it
    /// now stands.
    Delivered(SessionQuestion),
    /// Somebody got there first, or the question closed: the question as they
    /// left it, so whoever is answering can be told who and when.
    Resolved(SessionQuestion),
    /// This driver is not holding that question open.
    Unknown,
    /// This server holds no resident for the session.
    NoResident,
    /// The resident is still reading back its history after a restart, and
    /// does not yet know which questions it holds. Ask again in a moment.
    NotReady,
    /// The answer is not one the question could be given.
    Invalid(String),
    /// The write to claude failed; the question is still pending.
    Unreachable,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A working directory that has gone missing falls back to somewhere named,
    /// and never to nothing.
    ///
    /// `None` would leave the child inheriting the server's own directory, which
    /// on a developer's machine is the checkout of the server itself. The
    /// fallback has to be a path this code chose.
    #[test]
    fn a_session_whose_directory_is_gone_runs_at_home_rather_than_wherever_the_server_is() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let real = home.path().join("workspace");
        std::fs::create_dir_all(&real).expect("workspace");

        assert_eq!(
            session_workdir(&paths, Some(&real.to_string_lossy())),
            real,
            "a directory that exists is used"
        );
        assert_eq!(
            session_workdir(&paths, Some(&home.path().join("deleted").to_string_lossy())),
            home.path(),
            "one that does not falls back to home"
        );
        assert_eq!(session_workdir(&paths, None), home.path(), "and so does a session with none recorded");
        // A file is not a directory, and `current_dir` on one fails the spawn.
        let file = home.path().join("not-a-dir");
        std::fs::write(&file, b"x").expect("file");
        assert_eq!(session_workdir(&paths, Some(&file.to_string_lossy())), home.path());
    }
}
