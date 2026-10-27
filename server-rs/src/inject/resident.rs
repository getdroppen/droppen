//! One resident `claude` per session, fed message after message over stdin.
//!
//! The model this replaces spawned `claude --resume <id> --print -- <prompt>`
//! once per message and read the turn's end off the child's exit. That is a
//! clean thing to implement and a wrong thing to run: the CLI treats the end of
//! a `--print` turn as the end of its life, and gives whatever the turn left
//! running in the background a fixed grace period before collecting it. A user
//! whose agent dispatches long background work therefore had it cut off every
//! ten minutes, and nothing in remote-hand could see why.
//!
//! So a session is now a *process*:
//!
//! ```text
//! claude -p --input-format stream-json --output-format stream-json --verbose
//! ```
//!
//! …started once, held open by [`super::shim`], and handed one line of JSON per
//! message. The two events the old model conflated are now separate, and the
//! whole of this file is the consequence:
//!
//! * **a turn ends** when a `{"type":"result"}` line arrives. The composer
//!   unblocks, the phone is notified, the next queued message goes in.
//! * **the session dies** when the process exits. That is a failure only if a
//!   turn was in flight; an idle resident that went away is just an entry to
//!   clean up, and the next message starts a new one.
//!
//! # Who decides what
//!
//! Every mutation of a session's state happens in its driver task, and nowhere
//! else. The send path builds a [`Recipe`] — argv, environment, fingerprint,
//! all the things that need the account store and the uploads directory — and
//! posts it; the driver decides whether that message goes to stdin now, waits
//! in the queue, or needs a differently-configured process first. Putting the
//! decision anywhere else would mean reading a state the driver is free to
//! change in the same breath, and losing that race means two messages in one
//! turn, or a message written to a process that is on its way out.
//!
//! The one exception is the *first* spawn, which the send path does itself so
//! that a `claude` which cannot be launched at all is a refusal of the request
//! that tried, rather than a failure discovered later by somebody watching a
//! session. Respawns belong to the driver, because by then there is a turn to
//! report them against.
//!
//! # Surviving things
//!
//! Three of them, and they are the reason a resident is worth its complexity.
//!
//! * **A server restart.** The process is behind [`super::shim`], so killing
//!   this server does not touch it. The next boot re-attaches through the shim
//!   socket named in the journal, replays claude's whole stdout to work out
//!   where the session got to, and carries on — see [`Driver::attached`] and
//!   [`Driver::replay`].
//! * **claude falling over mid-turn.** A resume → fork ladder brings the
//!   conversation back and the queue is re-delivered into it, with a crash-loop
//!   breaker so a claude that cannot start at all is not restarted for ever.
//!   See [`Driver::recover`].
//! * **Background work.** A turn can end while claude is still minding tasks it
//!   started — the whole reason `--print` was wrong. Ending the process then
//!   would kill them, so the two things that would (a settings respawn and the
//!   optional idle shutdown) wait for the tasks to finish. See
//!   [`Driver::minding_background`].

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;
use tokio::sync::mpsc;

use crate::accounts::limits::{self, LimitClass, Verdict};
use crate::accounts::rest::{self, Observation};
use crate::accounts::rotation::{self, AheadGuards, AheadPlan, Plan, RotationOutcome, RotationTrigger, Wait};
use crate::accounts::store::{AccountProvider, AccountRef};
use crate::openapi_ext::Optional;
use crate::process_env::Env;
use crate::procs;
use crate::record::{Recorder, SpawnSummary, Stage};
use crate::sessions::{iso8601, InjectionRunState, SessionStatus, Source};

use super::questions::{self, PermissionRequest, QuestionState, SessionQuestion, Stored};
use super::receipts::ReceiptStatus;
use super::shim::{self, Link, Output, Sidecars, Spawn};
use super::{lock, new_uuid, runs, InjectError, InjectionRegistry, Run, RunState};

/// The mode flags every resident is started with.
///
/// `--verbose` is not decoration. claude 2.1.231 refuses `--output-format
/// stream-json` without it and exits before printing a line, so dropping it
/// turns every session into a launch failure.
const STREAM_FLAGS: [&str; 6] = ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"];

/// How long a polite shutdown is given before the ladder takes over.
///
/// Only reached on a respawn, and a respawn only ever happens from idle: claude
/// finishes what it is doing when its stdin closes, and what it is doing is
/// nothing.
const CLOSE_GRACE: Duration = Duration::from_secs(15);

/// How much of claude's last words a failed turn carries.
const STDERR_TAIL_LINES: usize = 6;

/// How much of an unparseable line is worth logging.
const LOG_LINE_CAP: usize = 400;

/// How much of a queued message the row carries.
///
/// A preview, not the prompt: this rides on every list row and every feed
/// frame, and a queue of hundred-thousand-character messages would be sent to a
/// phone once a second. The whole text is in the journal and, a turn later, in
/// the transcript.
const QUEUE_PREVIEW_CAP: usize = 200;

/// How long each rung of the crash-recovery ladder waits before trying again.
///
/// The whole of it runs before the breaker below gives up, which is the reason
/// the breaker counts to four and not three: with three strikes the thirty-second
/// rung is unreachable, and a claude that is briefly unspawnable — the seconds an
/// upgrade takes to swap the binary is the everyday case — would take the session
/// with it.
pub(super) const RECOVERY_BACKOFF: [Duration; 3] = [Duration::from_secs(1), Duration::from_secs(5), Duration::from_secs(30)];

/// How long a steered message's turn waits for claude to prove it is running.
///
/// The one number the merged/deferred ambiguity is decided by, and the reason
/// it can be this short: a message claude buffered is answered with a
/// `system:init` of its own *immediately* — the CLI reads the next line of
/// stdin the instant the previous turn's `result` goes out, and the init is the
/// first thing it writes. Two seconds is three orders of magnitude past that
/// and still fast enough that a queue behind an absorbed message does not look
/// stuck. See [`Driver::settle_delivered`].
pub(super) const STEER_SETTLE: Duration = Duration::from_secs(2);

/// How often a resident holding steered messages reads its transcript for them.
///
/// Only while one is outstanding, and only the bytes written since the last
/// look. A watched session hears sooner — its feed says when the file grows —
/// so this is the pace of the session list for a session nobody has open. See
/// [`Driver::absorb`].
pub(super) const ABSORB_POLL: Duration = Duration::from_secs(1);

/// The shortest a settle will ever wait before asking again.
///
/// Only reached by a CLI that is talking right at the edge of the window: the
/// re-arm is otherwise "however long is left of the silence", and that figure
/// goes to zero as the last line approaches the deadline. A floor rather than a
/// special case, because the thing being prevented is a spin and not a wrong
/// answer.
const SETTLE_RECHECK: Duration = Duration::from_millis(25);

/// How often a quota reading with headroom may wake the residents holding
/// their queues after a limit; see [`InjectionRegistry::recheck_holds`].
const READING_RECHECK_EVERY: Duration = Duration::from_secs(5);

/// How many deaths inside [`RECOVERY_WINDOW`] mean the session is not coming back.
const RECOVERY_STRIKES: usize = 4;

/// The window the breaker counts deaths in.
const RECOVERY_WINDOW: Duration = Duration::from_secs(300);

/// How recently a subagent transcript must have been written for claude to be
/// assumed still busy with background work.
///
/// The second signal, and only a corroborating one: the event stream is the
/// real answer — see [`Driver::background`] — and this exists for the case the
/// events cannot cover, which is a `task_started` that fell into a shim replay
/// gap. Kept short deliberately, because it is also paid by the ordinary case:
/// a turn that ran a *foreground* subagent leaves a fresh directory too, and
/// every second of this window is a second a settings change waits for nothing.
const SUBAGENT_QUIET: Duration = Duration::from_secs(30);

/// How long a transcript must have been untouched before it may be read as
/// evidence that no turn is in flight.
///
/// A prompt written into claude's stdin reaches the transcript a fraction of a
/// second later — measured at ~250 ms on 2.1.231 — so a file written moments ago
/// may simply not have caught up with the turn being asked about. Inside this
/// window [`Driver::transcript_is_between_turns`] declines to answer, and the
/// pessimistic reading of the stream stands.
const TRANSCRIPT_SETTLES_AFTER: Duration = Duration::from_secs(30);

// ------------------------------------------------------------ the fingerprint

/// What a resident was started with, and therefore what it can be handed.
///
/// Every field here is on the command line, which for a resident means fixed
/// for the process' whole life: a message that wants a different model cannot
/// simply be written to a process started under the old one. claude does have
/// `control_request` subtypes that change some of these in place, but they are
/// per-setting, versioned, and acknowledged even for values the CLI then
/// ignores — so the honest implementation is to compare and restart, and to
/// leave in-place switching to a phase that can verify it took.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(super) struct Fingerprint {
    /// `--model`, as the command line asked for it.
    pub model: Option<String>,
    /// The name the CLI answered under, when the process is the one being
    /// *described* rather than the one being asked for.
    ///
    /// One model, two names: a spawn says `--model fable` and `system:init`
    /// answers `claude-fable-5`. Both name the process that is running, and a
    /// send may carry either — the composer's own choice, or the name it read
    /// back out of the transcript — so both are compared against.
    ///
    /// Keeping only one of them is not a cosmetic loss. The driver used to
    /// replace the alias with the resolved name the moment `init` arrived,
    /// which made every send that still said `fable` look like a settings
    /// change: a flight recording of one real session shows eight turns and
    /// eight respawns, each one killing whatever the resident was minding in
    /// the background, and any message queued behind them failed outright
    /// because a message out of the journal has no environment to start a
    /// process with. Recording both names is what makes a resident a resident.
    ///
    /// Always `None` on a message — a send asks for a model, it cannot know
    /// what the CLI will call it — and ignored on that side by
    /// [`Fingerprint::satisfied_by`]. Absent from every record written before
    /// the second name was kept, which reads as "the CLI has not said yet".
    pub resolved: Option<String>,
    /// `--effort`.
    pub effort: Option<String>,
    /// `--permission-mode`.
    pub permission: Option<String>,
    /// The account whose token is in the environment; `None` is the root's own
    /// login. Not a flag, but the same kind of thing: fixed at spawn, and a
    /// message billed to another account needs its own process.
    pub account: Option<String>,
    /// *Which* credential that account held when this process started.
    ///
    /// The hash the store already keeps — of an OAuth token, or of an env
    /// block — and never a value; see
    /// [`crate::accounts::store::AccountsStore::credential_hash`].
    ///
    /// The account id is not enough, and the case that proved it is the whole
    /// reason this field exists: a user whose env account pointed at a base URL
    /// with one path segment too many fixed the block, and every *new* session
    /// then worked while the one they were sitting in kept failing. The
    /// resident had been started with the old environment hours earlier, the id
    /// had not changed, so nothing asked it to stop. Re-authorizing an OAuth
    /// account is the same story with a 401 at the end of it.
    ///
    /// Compared exactly, like the account beside it. A record written before
    /// this field existed has `None` here and will not match a message that
    /// knows the hash, which costs one respawn per resident that outlived the
    /// upgrade — once, and never again, because the spawn that replaces it
    /// records the hash.
    ///
    /// `None` for the local login, which has no stored credential at all: that
    /// fingerprint is exactly what it was.
    pub credential: Option<String>,
    /// The instant-chat persona and `--disallowedTools *`.
    pub chat: bool,
    /// Whether `--add-dir <uploads>` is on the command line.
    pub uploads: bool,
    /// Whether `--permission-prompt-tool stdio` and the plan-mode
    /// `--disallowedTools` are: whether this process can ask the user a
    /// question at all. See [`super::questions`].
    ///
    /// Written down and never compared. Every spawn this server makes has it,
    /// and a resident started before it existed is not restarted to get it:
    /// throwing away a warm session — and whatever it is minding in the
    /// background — for a capability its next message may never use is the
    /// trade [`Fingerprint::satisfied_by`] exists to refuse. It arrives with the
    /// next replacement the process gets for a reason of its own: a settings
    /// change, an account switch, a crash, an idle retirement. Absent from every
    /// record written before it, which reads as `false` — and is.
    pub questions: bool,
}

impl Fingerprint {
    /// Whether a message wanting `self` can go to a resident started with `live`.
    ///
    /// Every field is an equality test except the uploads directory, which is
    /// one-way on purpose. `--add-dir` only ever *widens* what claude may read
    /// without asking, so a resident that has it can take a message that does
    /// not need it; treating that as a mismatch would restart the process every
    /// time a conversation alternated between messages with and without an
    /// attachment — the common case, and the worst possible reason to throw a
    /// warm session away.
    ///
    /// `None` on the message side is *no opinion*, not "the default": a send
    /// that named no model, effort or permission is asking for the session as
    /// it stands, and reading it as a demand for factory settings would
    /// restart — or park behind background work — a process nobody asked to
    /// touch. The account is the exception, because `None` there is a real
    /// value (the local login, no token) and treating it as a wildcard would
    /// quietly keep billing an account the sender chose to drop.
    pub(super) fn satisfied_by(&self, live: &Fingerprint) -> bool {
        fn wants(asked: &Option<String>, running: &Option<String>) -> bool {
            asked.is_none() || asked == running
        }
        // The model is the one setting with two right answers, because the CLI
        // renames it: see [`Fingerprint::resolved`]. Asking for either name is
        // asking for the process that is running.
        (wants(&self.model, &live.model) || wants(&self.model, &live.resolved))
            && wants(&self.effort, &live.effort)
            && wants(&self.permission, &live.permission)
            && self.account == live.account
            && self.credential == live.credential
            && self.chat == live.chat
            && (!self.uploads || live.uploads)
        // `questions` deliberately absent: see the field.
    }

    /// The same question, asked of a process whose settings may not be recorded.
    ///
    /// `None` is *unknown*, and unknown accepts. It has to: the alternative is
    /// what shipped, which read a record written before the fingerprint field
    /// existed as "a process started with no flags at all" — a description
    /// nothing but a flagless message satisfies, because the account is compared
    /// exactly and the wildcards are all on the message's side. A resident that
    /// outlived the upgrade therefore refused every message it was holding
    /// ("the settings this message needs were lost when the server restarted")
    /// and was then thrown away by the first fresh one, background work and all.
    ///
    /// Accepting is not a hole in the check, because unknown does not last: the
    /// first message that brings a recipe says what the process is, and
    /// [`Driver::take_settings`] writes that back to the journal. What is
    /// deliberately not done is *guessing* — a guess that happens to be wrong is
    /// the failure this replaces, in the other direction.
    fn accepted_by(&self, live: Option<&Fingerprint>) -> bool {
        match live {
            Some(live) => self.satisfied_by(live),
            None => true,
        }
    }

    /// Whether a message wanting `self` would fit a process running `live` if
    /// only the login were set aside — the shape of a message an account
    /// rotation, or a binding changed by hand, pointed at another account
    /// while the process it was queued behind kept running on the old one.
    fn fits_but_for_login(&self, live: &Fingerprint) -> bool {
        let unlogged = |fingerprint: &Fingerprint| Fingerprint {
            account: None,
            credential: None,
            ..fingerprint.clone()
        };
        self.account.is_some() && unlogged(self).satisfied_by(&unlogged(live))
    }

    /// Why a message wanting `self` cannot go to a process running `live`, in
    /// words the person who sent it will recognise.
    ///
    /// For the report and nothing else — [`Fingerprint::satisfied_by`] is the
    /// decision — which is why it can afford to be approximate at the edges: a
    /// caller only reaches here having already been told the two differ.
    fn differences(&self, live: &Fingerprint) -> String {
        fn named(value: &Option<String>) -> &str {
            value.as_deref().unwrap_or("none")
        }
        fn login(value: &Option<String>) -> String {
            match value {
                Some(account) => format!("the {account} account"),
                // Not an account id: the root's own login, which is a real
                // choice and not the absence of one.
                None => "the local login".to_string(),
            }
        }
        /// Both of a running process' names, when the CLI gave it two.
        fn running_model(live: &Fingerprint) -> String {
            match (&live.model, &live.resolved) {
                (Some(asked), Some(answered)) if asked != answered => format!("{asked} (answering as {answered})"),
                (Some(name), _) | (None, Some(name)) => name.clone(),
                (None, None) => "none".to_string(),
            }
        }
        let mut parts = Vec::new();
        if self.model.is_some() && self.model != live.model && self.model != live.resolved {
            parts.push(format!("model {} rather than {}", named(&self.model), running_model(live)));
        }
        for (setting, asked, running) in [
            ("effort", &self.effort, &live.effort),
            ("permission mode", &self.permission, &live.permission),
        ] {
            if asked.is_some() && asked != running {
                parts.push(format!("{setting} {} rather than {}", named(asked), named(running)));
            }
        }
        if self.account != live.account {
            parts.push(format!("{} rather than {}", login(&self.account), login(&live.account)));
        } else if self.credential != live.credential {
            // Same login, different credential: the account was re-authorized
            // or its environment block edited after this process started.
            parts.push(format!(
                "{}'s current credentials, which this process did not start with",
                login(&self.account)
            ));
        }
        if self.chat != live.chat {
            parts.push(if self.chat { "the chat persona" } else { "no chat persona" }.to_string());
        }
        if self.uploads && !live.uploads {
            parts.push("access to the uploads directory".to_string());
        }
        if parts.is_empty() {
            // Not reachable from a real mismatch, and not worth an assertion:
            // an empty reason in a failure report reads as a bug in the report.
            return "different settings".to_string();
        }
        parts.join(", ")
    }
}

/// Everything needed to start a resident, resolved where it can be.
///
/// Built on the send path because that is the only place with the account
/// store, the uploads directory and the session's working directory in hand;
/// carried by the message so that a queued message which does not fit the
/// running process still knows what process it does fit.
#[derive(Debug, Clone)]
pub(super) struct Recipe {
    /// Everything after the session selector: model, effort, permission,
    /// `--add-dir`, the chat persona. The selector itself is the driver's,
    /// because a respawn resumes an id the first spawn may not have known.
    pub args: Vec<String>,
    /// The whole environment, applied after an `env_clear`.
    pub env: Env,
    /// Where claude runs.
    pub workdir: Option<PathBuf>,
    /// What [`Fingerprint::satisfied_by`] compares.
    pub fingerprint: Fingerprint,
}

// ---------------------------------------------------------------- the mirror

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
    pub queued_messages: Vec<crate::sessions::QueuedMessage>,
    /// How many background tasks claude is still minding.
    ///
    /// Deliberately *not* a phase. A turn that dispatched background work ends
    /// like any other — the composer unblocks, the next message goes in — and
    /// the tasks outlive it. What this count changes is only what may end the
    /// *process*: see [`Driver::minding_background`].
    pub background: usize,
    /// What those tasks are, oldest first, capped at [`TASKS_ON_THE_WIRE`].
    ///
    /// The count above stays the authoritative one: this is the description of
    /// the work, and a row that showed a hundred of anything is already telling
    /// the user something has gone wrong.
    pub tasks: Vec<crate::sessions::BackgroundTask>,
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
}

impl ResidentState {
    /// Whether claude is looking after work that outlived its turn.
    pub fn minding_background(&self) -> bool {
        self.background > 0
    }
}

/// What the send path and the routes hold on to.
pub(super) struct Handle {
    tx: mpsc::UnboundedSender<Cmd>,
    shared: Arc<Mutex<ResidentState>>,
}

impl Handle {
    fn state(&self) -> ResidentState {
        self.shared.lock().expect("resident mirror poisoned").clone()
    }

    /// The mirror in the shape the API speaks in.
    pub(super) fn info(&self) -> crate::sessions::ResidentInfo {
        self.state().into()
    }
}

impl From<ResidentState> for crate::sessions::ResidentInfo {
    fn from(state: ResidentState) -> Self {
        crate::sessions::ResidentInfo {
            phase: match state.phase {
                Phase::Starting => crate::sessions::ResidentPhase::Starting,
                Phase::Working => crate::sessions::ResidentPhase::Working,
                Phase::Idle => crate::sessions::ResidentPhase::Idle,
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
            turn_started_at: crate::openapi_ext::Nullable(state.turn_started_at),
            asking: state.question.is_some(),
        }
    }
}

/// What the driver task accepts.
enum Cmd {
    /// A message for this session, with the process it wants.
    Deliver {
        pending: Box<Pending>,
        /// Answered once the message is in the journal, which is what the send
        /// route waits for before it says `200`; see
        /// [`InjectionRegistry::deliver`].
        recorded: tokio::sync::oneshot::Sender<()>,
    },
    /// Take a message back out of the queue before it goes in.
    Drop {
        /// Which one, by [`Pending::id`].
        qid: String,
        /// Where the answer goes, because the caller is an HTTP handler.
        reply: tokio::sync::oneshot::Sender<QueueOutcome>,
    },
    /// Hand a queued message to claude now, ahead of the turn in flight.
    Steer {
        /// Which one, by [`Pending::id`].
        qid: String,
        reply: tokio::sync::oneshot::Sender<QueueOutcome>,
    },
    /// Stop the turn in flight, politely first.
    Interrupt,
    /// Stop one background task, by the CLI's own `task_id`.
    StopTask {
        /// [`crate::sessions::BackgroundTask::id`], which is claude's id for it.
        task_id: String,
    },
    /// A rung of the interrupt ladder came due.
    Escalate { turn: u64, rung: u8 },
    /// Ask a held queue's question again, because something changed that
    /// could change the answer. See [`Driver::recheck_hold`].
    Recheck,
    /// A steered message's turn has waited long enough to be judged.
    Settle {
        /// Which turn, by [`Turn::seq`]: a settle that comes due after its turn
        /// ended must not touch the turn that replaced it.
        turn: u64,
    },
    /// Look in the transcript for steered messages claude has already taken
    /// into the turn in flight. See [`Driver::absorb`].
    Absorb {
        /// True when the session's feed sent it, having just put the grown
        /// transcript on screen; false for the driver's own poll.
        nudged: bool,
    },
    /// The user's answer to a question claude is blocked on, or their refusal
    /// to give one.
    Answer {
        /// Which question, by the CLI's request id.
        request_id: String,
        reply: Reply,
        /// Who is answering, when the server knows.
        by: Option<String>,
        respond: tokio::sync::oneshot::Sender<AnswerOutcome>,
    },
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
    /// the restart, see [`Driver::restart_for_steer`].
    NeedsRestart {
        /// What the message wants that the running process does not have, in
        /// the words of [`Fingerprint::differences`].
        ///
        /// Carried rather than re-derived at the route, which holds neither
        /// fingerprint: the refusal is only actionable if it says which setting
        /// is in the way.
        differences: String,
    },
    /// The write to claude failed; the message is still queued.
    Unreachable,
}

/// One message waiting for its turn.
struct Pending {
    /// What the queue routes name this message by.
    ///
    /// Minted on the way in and carried to the journal, so an id a client read
    /// off a row still names the same message after a server restart. Never
    /// reused: the id of a message that has started is simply not found, which
    /// is what makes "delete the thing I just sent" fail safely rather than
    /// deleting whatever took its place.
    id: String,
    /// The prompt as claude will see it, attachment suffix included.
    text: String,
    /// The process this message needs, which may not be the one that is running.
    fingerprint: Fingerprint,
    /// How to start that process, for a message this server was handed.
    ///
    /// `None` for a message read back out of the journal after a restart. The
    /// record keeps what a message *wanted* but not the environment it would
    /// need — that carries the account's token — so such a message can only be
    /// given to the process that is already running, which is exactly what
    /// adoption checks before it keeps one.
    recipe: Option<Recipe>,
    /// How many automatic account switches the chain this message belongs to
    /// has had: `0` for anything a person sent, `n` for the continuation the
    /// `n`th switch queued. The turn it opens carries it, so a limit on that
    /// turn knows how close the chain is to [`rotation::ROTATION_CAP`].
    rotation_attempt: u8,
    /// Set on the continuation a rotation queued: what is needed to say which
    /// background tasks its respawn cut short, if it had to wait for them and
    /// they did not all finish. Journalled with the message. See
    /// [`Continuation`].
    continuation: Option<Continuation>,
}

/// What a continuation remembers about the switch that queued it.
///
/// A switch that waits for background work does not know when it is queued
/// what the respawn will interrupt; the respawn does. So the text is kept in
/// its parts and rewritten then, and the log entry corrected to match. Kept in
/// the journal beside the message, so a restart in the middle of the wait
/// loses neither.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Continuation {
    /// The rotation log entry, when one was written.
    record: Option<i64>,
    /// The account moved off and the one moved to, as the text names them.
    from: String,
    to: String,
    /// [`rotation::reason`] and [`rotation::until_text`], already worded.
    reason: String,
    until: Option<String>,
}

impl Pending {
    /// A message this server has just been handed, under a fresh queue id.
    #[cfg(test)]
    fn new(text: &str, recipe: Recipe) -> Pending {
        Pending::with_id(new_uuid(), text, recipe)
    }

    /// The same, under a queue id the send path already chose — the one a
    /// client's receipt follows; see [`super::receipts`].
    fn with_id(id: String, text: &str, recipe: Recipe) -> Pending {
        Pending {
            id,
            text: text.to_string(),
            fingerprint: recipe.fingerprint.clone(),
            recipe: Some(recipe),
            rotation_attempt: 0,
            continuation: None,
        }
    }

    /// This message as the journal holds it.
    ///
    /// `steered` is the driver's, not the message's: it says which side of the
    /// stdin write this entry was on when the record was rewritten.
    fn recorded(&self, steered: bool) -> runs::Queued {
        runs::Queued {
            id: self.id.clone(),
            text: self.text.clone(),
            fingerprint: self.fingerprint.clone(),
            steered,
            rotation_attempt: self.rotation_attempt,
            continuation: self.continuation.clone(),
        }
    }

    /// This message as a row shows it.
    fn preview(&self, steered: bool) -> crate::sessions::QueuedMessage {
        crate::sessions::QueuedMessage {
            id: self.id.clone(),
            text: clip_to(&self.text, QUEUE_PREVIEW_CAP),
            steered,
        }
    }
}

// ------------------------------------------------------------- the send path

/// What a delivery did, for the caller to report.
pub(super) struct Delivery {
    /// True when the message went in behind a turn that is still running.
    pub queued: bool,
}

/// How long a send may wait for its resident to journal the message before
/// the wait is logged. Normally it is milliseconds; the client calls a send
/// it heard nothing about for 30 s "unknown", so a slow journal is worth seeing.
const SLOW_JOURNAL: Duration = Duration::from_secs(2);

/// Wait for a resident's driver to say a message is on disk — exactly the
/// wait [`InjectionRegistry::deliver`] has always made (it is what keeps an
/// accepted message from being lost to a kill) — logging it when it takes
/// longer than [`SLOW_JOURNAL`]: once when it passes that mark, so a wait
/// that never ends is seen too, and again with the total when it ends.
async fn wait_recorded(
    session: &str,
    queued: bool,
    recorded: tokio::sync::oneshot::Receiver<()>,
) -> Result<(), tokio::sync::oneshot::error::RecvError> {
    let started = tokio::time::Instant::now();
    tokio::pin!(recorded);
    tokio::select! {
        result = &mut recorded => return result,
        () = tokio::time::sleep(SLOW_JOURNAL) => {}
    }
    tracing::warn!(
        session,
        queued,
        waited_ms = started.elapsed().as_millis() as u64,
        "a send is still waiting for its resident to journal the message"
    );
    let result = recorded.await;
    tracing::warn!(
        session,
        queued,
        waited_ms = started.elapsed().as_millis() as u64,
        recorded = result.is_ok(),
        "a send waited long for its resident to journal the message"
    );
    result
}

impl InjectionRegistry {
    /// Whether this server holds a resident process for a claude session.
    ///
    /// The composer gate asks this. A session whose CLI is registered as live
    /// is normally refused, because writing into a conversation somebody's own
    /// terminal has open interleaves two agents in one transcript — and a
    /// resident registers itself exactly the same way, because it *is* a real
    /// `claude`. Without this the first turn would be the last one this server
    /// could send.
    pub fn has_resident(&self, source: Source, id: &str) -> bool {
        if source == Source::Codex {
            return self.codex_handle(id).is_some_and(|h| h.visible());
        }
        source == Source::Claude && self.residents.lock().expect("residents poisoned").contains_key(id)
    }

    /// How a session's resident is doing, for a caller that wants the detail.
    pub fn resident_state(&self, source: Source, id: &str) -> Option<ResidentState> {
        if source == Source::Codex {
            return self.codex_handle(id).filter(|h| h.visible()).map(|h| h.state());
        }
        if source != Source::Claude {
            return None;
        }
        Some(self.residents.lock().expect("residents poisoned").get(id)?.state())
    }

    /// Hand a message to this session's resident, starting one if needed.
    ///
    /// The spawn happens here, and only the first one: a `claude` that is
    /// missing, unexecutable or refused by the shim is a failure of *this*
    /// request and is reported as one. Everything afterwards is the driver's.
    ///
    /// `qid` is the queue id to deliver under, when the send path chose one;
    /// `None` mints one.
    ///
    /// # Returns only once the message is on disk
    ///
    /// The caller answers `200` the moment this returns, and a server killed
    /// the instant after has nothing to hand the next one but the journal. A
    /// message that starts a resident is in the record it is born with (see
    /// [`InjectionRegistry::start_resident`]). One posted to a resident that is
    /// already running is the driver's to write down, so this waits until it
    /// has — the same wait the queue routes and the codex send already make.
    /// Posting and returning, as this once did, answered `200` while the message
    /// was still sitting in the driver's inbox, and a `SIGKILL` in that window
    /// lost a message the user had been told was accepted.
    pub(super) async fn deliver(self: &Arc<Self>, id: &str, text: &str, recipe: Recipe, qid: Option<String>) -> Result<Delivery, InjectError> {
        let qid = qid.unwrap_or_else(new_uuid);
        // Twice at most: the second pass is only for a driver that ended with
        // this message still in its inbox, and by the time that is known its
        // entry is gone, so the second pass starts a resident of its own.
        for _ in 0..2 {
            // Serialized across sessions rather than per session, because what
            // is being protected is the gap between "no resident in the map"
            // and "one in the map": a second send arriving inside it would start
            // a second claude against the same transcript. Spawns are rare and
            // take milliseconds, so a per-key lock map would cost more than it
            // saves.
            let gate = self.spawn_gate.lock().await;
            let Some((queued, recorded)) = self.post_to_existing(id, &qid, text, &recipe) else {
                self.clone().start_resident(id, Box::new(Pending::with_id(qid, text, recipe))).await?;
                return Ok(Delivery { queued: false });
            };
            // The message is in a driver's inbox, so the gap the gate guards is
            // closed. Waiting for the journal under it would hold every other
            // session's sends behind one driver that is busy recovering.
            drop(gate);
            if wait_recorded(id, queued, recorded).await.is_ok() {
                return Ok(Delivery { queued });
            }
            // Dropped unanswered: the driver ended before it reached this
            // message, which is therefore neither queued nor written down
            // anywhere. Silently losing it is what used to happen here.
            tracing::warn!(session = %id, "a resident ended before it recorded a message sent to it; sending it again");
        }
        Err(InjectError::message("claude ended before this message could be recorded; send it again"))
    }

    /// Post to the resident already running, if there is one that will take it,
    /// and say whether the message waits behind a turn and when it is on disk.
    ///
    /// `None` means "start one": either there was no entry, or the driver
    /// behind it has stopped reading — which is the moment between the process
    /// exiting and the driver clearing its own entry.
    fn post_to_existing(&self, id: &str, qid: &str, text: &str, recipe: &Recipe) -> Option<(bool, tokio::sync::oneshot::Receiver<()>)> {
        let mut residents = self.residents.lock().expect("residents poisoned");
        let handle = residents.get(id)?;
        let pending = Box::new(Pending::with_id(qid.to_string(), text, recipe.clone()));
        let (recorded, on_disk) = tokio::sync::oneshot::channel();
        if handle.tx.send(Cmd::Deliver { pending, recorded }).is_err() {
            // The driver is gone and its entry is debris. Clearing it here
            // rather than waiting for the driver's own cleanup is what saves
            // the caller from having to retry.
            residents.remove(id);
            return None;
        }
        Some((handle.state().phase != Phase::Idle, on_disk))
    }

    /// Start a resident for this session and give it the first message.
    async fn start_resident(self: Arc<Self>, id: &str, first: Box<Pending>) -> Result<(), InjectError> {
        // The whole-machine claim, held for as long as the process lives rather
        // than for one turn: a resident *is* the session's occupancy, and a
        // second instance starting its own against the same transcript is the
        // interleaving this lock exists to stop.
        let claim = match lock::acquire(&self.lock_dir, Source::Claude, id) {
            Ok(lock::Outcome::Acquired(claim)) => claim,
            Ok(lock::Outcome::Busy { holder }) => return Err(InjectError::message(super::busy_message(holder))),
            Err(error) => return Err(InjectError::Internal(format!("cannot claim this session: {error}"))),
        };

        // A session that already has a transcript is resumed; one that does not
        // is being created, and `--session-id` is what makes the id the client
        // was already told about the one claude writes under.
        let selector = match crate::sessions::claude::find_claude_session_file(&self.paths, id).await {
            Some(_) => Selector::Resume,
            None => Selector::Fresh,
        };
        // A message this server was handed always brings a recipe; the only
        // messages that do not are the ones read back out of the journal, and
        // those are never the reason a process is being started.
        let Some(recipe) = first.recipe.clone() else {
            return Err(InjectError::Internal("a message with no recipe cannot start a resident".to_string()));
        };
        // Opened before the spawn, so the very first thing in a session's
        // recording is the process that started it. A recorder that cannot be
        // opened is one that does nothing, so there is no failure to handle.
        let recorder = self.recorders.open(id);
        // The record is born holding this message, so the window in which a send
        // has been accepted and nothing on disk knows about it is the spawn
        // itself rather than the whole of the CLI's start-up.
        let spawned = self.spawn_link(id, &recipe, selector, None, &[first.recorded(false)], &recorder).await?;

        let shared = Arc::new(Mutex::new(ResidentState {
            phase: Phase::Starting,
            stalled: false,
            queued: 1,
            queued_messages: vec![first.preview(false)],
            background: 0,
            tasks: Vec::new(),
            awaiting_background: false,
            turn_started_at: Some(iso8601(SystemTime::now())),
            question: None,
        }));
        let (tx, rx) = mpsc::unbounded_channel();
        self.residents.lock().expect("residents poisoned").insert(
            id.to_string(),
            Handle {
                tx: tx.clone(),
                shared: shared.clone(),
            },
        );
        // The turn is visible from the moment the caller is told it was
        // accepted, and not from the moment claude gets around to saying it is
        // listening. Nothing may be written to a process before `system:init`,
        // so the driver cannot open the real turn yet — and a session that reads
        // as idle for the second or two a cold CLI takes to boot is a message
        // that, as far as the phone can tell, vanished. Replaced by the driver's
        // own entry as soon as there is one, and by the failure if the process
        // dies before there is.
        self.runs
            .lock()
            .expect("injections poisoned")
            .insert(self.key_of(Source::Claude, id), running_turn(id));

        let driver = Driver {
            registry: self.clone(),
            id: id.to_string(),
            sidecars: spawned.sidecars,
            link: spawned.link,
            record: spawned.record,
            recorder,
            live: Some(recipe.fingerprint.clone()),
            recipe: Some(recipe),
            tx,
            rx,
            shared,
            claim: Some(claim),
            queue: VecDeque::from([*first]),
            delivered_ahead: VecDeque::new(),
            absorb: AbsorbWatch::default(),
            turn: None,
            ready: false,
            heard: Instant::now(),
            replaying: false,
            replay_open: false,
            replay_marked: false,
            pending_turn: None,
            idle_since: None,
            seq: 0,
            fatal: None,
            background: BackgroundTasks::default(),
            awaiting_background: false,
            awaiting_since: None,
            deaths: VecDeque::new(),
            rung: 0,
            recovering: false,
            retiring: false,
            limit_hint: limits::HintSlot::default(),
            ahead_noted: None,
            hold: None,
            asked: Vec::new(),
            replay_asked: Vec::new(),
            replay_refusals: Vec::new(),
            replay_declines: Vec::new(),
            replay_gapped: false,
            draft: self.draft_writer(id),
            phase_moved: std::sync::atomic::AtomicBool::new(false),
        };
        tokio::spawn(driver.run());
        Ok(())
    }

    /// Start one shim-held claude and record it, without touching the map.
    ///
    /// Shared by the first spawn and by the driver's respawn, which is what
    /// keeps the journal record and the sidecar naming identical between them.
    async fn spawn_link(
        &self,
        id: &str,
        recipe: &Recipe,
        selector: Selector,
        retiring: Option<&str>,
        queued: &[runs::Queued],
        recorder: &Recorder,
    ) -> Result<Spawned, InjectError> {
        // On a fresh install no attachment has created this directory yet.
        // Do not claim the capability in the journal if preparing it failed.
        if recipe.fingerprint.uploads {
            tokio::fs::create_dir_all(self.paths.uploads_dir())
                .await
                .map_err(|error| InjectError::message(format!("cannot prepare the uploads directory: {error}")))?;
        }
        // Fresh sidecars for every spawn, never reused. The outgoing shim
        // unlinks its socket path on its way out and only checks that the path
        // still names *a* socket, so a replacement that bound the same path
        // would have it unlinked from under it.
        let sidecars = Sidecars::new(self.shim_dir.clone(), new_uuid());
        let mut args: Vec<String> = STREAM_FLAGS.iter().map(|flag| (*flag).to_string()).collect();
        match selector {
            Selector::Fresh => args.extend(["--session-id".to_string(), id.to_string()]),
            Selector::Resume => args.extend(["--resume".to_string(), id.to_string()]),
            // The second rung of the recovery ladder. claude answers under a new
            // id, which arrives in `system:init` and is followed by
            // [`Driver::adopt`] like any other fork — the one place in this file
            // that has always had to cope with the CLI renaming a session.
            Selector::Fork => args.extend(["--resume".to_string(), id.to_string(), "--fork-session".to_string()]),
        }
        args.extend(recipe.args.iter().cloned());

        let link = shim::spawn_via_shim(Spawn {
            sidecars: &sidecars,
            program: "claude",
            args: &args,
            workdir: recipe.workdir.as_deref(),
            env: &recipe.env,
            buffer_bytes: None,
        })
        .await
        .map_err(|error| InjectError::Message(format!("failed to launch claude: {error}")))?;

        let record = new_uuid();
        // The outgoing record goes only once the incoming process exists, so a
        // crash in between leaves something to clean up rather than nothing.
        if let Some(retiring) = retiring {
            self.journal.remove(retiring);
        }
        self.journal.add(runs::RunRecord {
            run_id: record.clone(),
            source: Source::Claude,
            session_id: id.to_string(),
            cwd: recipe.workdir.as_ref().map(|dir| dir.to_string_lossy().into_owned()),
            cli_pid: link.claude_pid(),
            started_at: iso8601(std::time::SystemTime::now()),
            pid_token: procs::start_token(link.claude_pid()),
            resident: true,
            shim: Some(sidecars.id().to_string()),
            fingerprint: Some(recipe.fingerprint.clone()),
            // A process that has not been written to yet has no turn in flight,
            // whatever the one it replaces was in the middle of.
            turn_at: None,
            // What it does inherit is the backlog: the messages that made this
            // spawn happen, and that this process exists to answer.
            queued: queued.to_vec(),
            codex: None,
            codex_account: None,
            hold: None,
        });
        tracing::info!(session = %id, pid = link.claude_pid(), shim = %sidecars.id(), ?selector, "started a resident claude");
        // The recording gets the *summary*, never the recipe: `recipe.env`
        // holds the account's token, and the type this takes has nowhere to put
        // one. See [`crate::record`].
        recorder.spawned(SpawnSummary {
            selector: selector.name(),
            pid: link.claude_pid(),
            shim: sidecars.id(),
            cwd: recipe.workdir.as_deref(),
            model: recipe.fingerprint.model.as_deref(),
            effort: recipe.fingerprint.effort.as_deref(),
            permission: recipe.fingerprint.permission.as_deref(),
        });
        Ok(Spawned { link, sidecars, record })
    }

    /// Ask every resident holding its queue after a limit to look again —
    /// something that could free an account for it has just happened. A
    /// resident holding nothing ignores it.
    pub(crate) fn recheck_holds(&self) {
        let residents = self.residents.lock().expect("residents poisoned");
        for handle in residents.values() {
            let _ = handle.tx.send(Cmd::Recheck);
        }
    }

    /// A claude session's transcript has grown, and its feed has just sent it.
    ///
    /// Its resident looks for steered messages claude folded into the turn in
    /// flight now rather than at its next poll — see `Driver::absorb` — so the
    /// message leaves the queue on screen in the same moment it appears in the
    /// transcript. Cheap for a resident with nothing steered: it returns at once.
    pub(crate) fn transcript_grew(&self, source: Source, id: &str) {
        if source != Source::Claude {
            return;
        }
        if let Some(handle) = self.residents.lock().expect("residents poisoned").get(id) {
            let _ = handle.tx.send(Cmd::Absorb { nudged: true });
        }
    }

    /// [`InjectionRegistry::recheck_holds`] for a quota reading with headroom,
    /// which arrives once per turn per session: at most once every
    /// [`READING_RECHECK_EVERY`], because the watchdog's tick is the net under
    /// it anyway.
    fn recheck_holds_after_reading(&self) {
        let mut last = self.reading_recheck.lock().expect("reading recheck poisoned");
        if last.is_some_and(|at| at.elapsed() < READING_RECHECK_EVERY) {
            return;
        }
        *last = Some(Instant::now());
        drop(last);
        self.recheck_holds();
    }

    /// Take a message back out of a session's queue before it goes in.
    ///
    /// Asked of the driver rather than done to the queue directly, for the
    /// reason every other decision in this file is the driver's: the queue is
    /// its state, and a route that reached into it would be racing the pump
    /// that is about to write its head to claude.
    pub(super) async fn drop_queued(&self, id: &str, qid: &str) -> QueueOutcome {
        self.ask(id, |reply| Cmd::Drop { qid: qid.to_string(), reply }).await
    }

    /// Hand a queued message to claude now, ahead of the turn in flight.
    pub(super) async fn steer_queued(&self, id: &str, qid: &str) -> QueueOutcome {
        self.ask(id, |reply| Cmd::Steer { qid: qid.to_string(), reply }).await
    }

    /// Put a question to a session's driver and wait for its answer.
    ///
    /// The sender is cloned out from under the map lock before anything is
    /// awaited: holding the residents lock across an await would block every
    /// send in the server behind one driver's inbox.
    async fn ask(&self, id: &str, build: impl FnOnce(tokio::sync::oneshot::Sender<QueueOutcome>) -> Cmd) -> QueueOutcome {
        let tx = {
            let residents = self.residents.lock().expect("residents poisoned");
            match residents.get(id) {
                Some(handle) => handle.tx.clone(),
                None => return QueueOutcome::NoResident,
            }
        };
        let (reply, answer) = tokio::sync::oneshot::channel();
        if tx.send(build(reply)).is_err() {
            // The driver has stopped reading, which is the moment between the
            // process exiting and its entry being cleared. Nothing is queued
            // there any more, whatever the map still says.
            return QueueOutcome::NoResident;
        }
        answer.await.unwrap_or(QueueOutcome::NoResident)
    }

    /// Stop what a resident is doing. `false` when there is nothing to stop.
    ///
    /// Only posts the request: the ladder that follows is timed by the driver,
    /// because only the driver knows whether the turn ended in between.
    ///
    /// `Starting` counts, and that is not a technicality. The window between a
    /// send being accepted and `system:init` arriving is exactly the window a
    /// cold CLI, a bad `--resume` id or a wedged launch sits in — the times a
    /// user is *most* likely to press Stop — and refusing it there left the only
    /// escape being to wait out a process that was never going to answer.
    pub(super) fn interrupt_resident(&self, id: &str) -> bool {
        let residents = self.residents.lock().expect("residents poisoned");
        let Some(handle) = residents.get(id) else { return false };
        if handle.state().phase == Phase::Idle {
            return false;
        }
        handle.tx.send(Cmd::Interrupt).is_ok()
    }

    /// Ask a resident to stop one background task.
    ///
    /// Checked against the list this server publishes rather than sent blind.
    /// claude answers `stop_task` with success for a task it cannot find, so a
    /// blind pass-through would report "stopped" for a typo, a stale row, or a
    /// task that finished while the request was in flight — three different
    /// things that all need to read as *not what you asked for*. The mirror is
    /// the same list the client's button was drawn from, so agreeing with it is
    /// the check that matches what the user was looking at.
    ///
    /// A task the mirror does not name is [`StopTaskOutcome::NoSuchTask`] even
    /// when claude might still know it — the honest report of a race, not a
    /// claim that the task never existed.
    pub(super) fn stop_resident_task(&self, id: &str, task_id: &str) -> StopTaskOutcome {
        let residents = self.residents.lock().expect("residents poisoned");
        let Some(handle) = residents.get(id) else {
            return StopTaskOutcome::NoResident;
        };
        if !handle.state().tasks.iter().any(|task| task.id == task_id) {
            return StopTaskOutcome::NoSuchTask;
        }
        if handle
            .tx
            .send(Cmd::StopTask {
                task_id: task_id.to_string(),
            })
            .is_err()
        {
            // The driver has stopped reading, which is the moment between the
            // process exiting and its entry being cleared. Its tasks went with
            // it, whatever the mirror still says.
            return StopTaskOutcome::NoResident;
        }
        StopTaskOutcome::Asked
    }

    /// The question a claude session's resident is blocked on, by request id:
    /// the oldest, when it is somehow holding more than one.
    ///
    /// Read off the mirror, so it answers what was true a moment ago. Every
    /// caller goes on to [`InjectionRegistry::answer_question`], which asks the
    /// driver and is told the truth.
    pub fn open_question(&self, id: &str) -> Option<String> {
        self.residents.lock().expect("residents poisoned").get(id)?.state().question
    }

    /// Answer a question claude is blocked on, or dismiss it.
    ///
    /// The driver does the whole of it — the compare-and-set on the record and
    /// the write to claude's stdin — because it is the one task that knows
    /// whether the question is still open: a question withdrawn, or a turn
    /// stopped, in the moment the request was in flight is answered
    /// [`AnswerOutcome::Unknown`] rather than written into a turn that has
    /// moved on.
    pub async fn answer_question(&self, id: &str, request_id: &str, reply: Reply, by: Option<String>) -> AnswerOutcome {
        let tx = {
            let residents = self.residents.lock().expect("residents poisoned");
            match residents.get(id) {
                Some(handle) => handle.tx.clone(),
                None => return AnswerOutcome::NoResident,
            }
        };
        let (respond, answer) = tokio::sync::oneshot::channel();
        let command = Cmd::Answer {
            request_id: request_id.to_string(),
            reply,
            by,
            respond,
        };
        if tx.send(command).is_err() {
            return AnswerOutcome::NoResident;
        }
        answer.await.unwrap_or(AnswerOutcome::NoResident)
    }
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

/// Which session a spawn asks claude for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selector {
    /// `--session-id`: a session that has no transcript yet, named in advance so
    /// the id the client was told is the one claude writes under.
    Fresh,
    /// `--resume`: carry on the transcript that is already there.
    Resume,
    /// `--resume --fork-session`: carry the history into a *new* id, for a
    /// resume that keeps failing before it can say it is listening.
    Fork,
}

impl Selector {
    /// The name a recording calls this by.
    fn name(self) -> &'static str {
        match self {
            Selector::Fresh => "fresh",
            Selector::Resume => "resume",
            Selector::Fork => "fork",
        }
    }
}

impl InjectionRegistry {
    /// Take over a resident this server did not start.
    ///
    /// The other half of [`InjectionRegistry::start_resident`], for a boot that
    /// found a live `claude` in the journal. Everything the driver needs is
    /// rebuilt from what survived: the link from the shim socket, the settings
    /// from the record, the session's state from the replay the attach triggers.
    ///
    /// The claim is the caller's to have taken — adoption takes it before
    /// dialling, because a socket that does not answer still leaves a process
    /// running that nobody else may talk to.
    pub(super) fn attached(self: &Arc<Self>, record: &runs::RunRecord, sidecars: Sidecars, link: Link, claim: lock::InjectLock) {
        // A record from before the fingerprint was written down says *unknown*,
        // and is carried through as unknown rather than as the default. The
        // default is "no flags at all", which is a real and very specific
        // description — one that nothing but a flagless message fits — so
        // reading it out of a record that says nothing is not caution, it is a
        // wrong answer that then throws the session's whole backlog away. See
        // [`Fingerprint::accepted_by`] for what unknown does instead, and
        // [`Driver::take_settings`] for how long it lasts.
        let live = record.fingerprint.clone();
        let Inherited { delivered_ahead, queue } = inherited_queue(&record.session_id, &record.queued, live.as_ref());
        let outstanding = delivered_ahead.len() + queue.len();
        // The ones that did not survive are the tail past the kept prefix: a
        // client that named one is told it failed rather than left waiting.
        for lost in record.queued.iter().skip(outstanding) {
            self.note_receipt(Source::Claude, &record.session_id, &lost.id, ReceiptStatus::Failed);
        }
        let shared = Arc::new(Mutex::new(ResidentState {
            // Not `Working`, whatever the replay is about to say: nothing may be
            // written to the process until an `init` has gone past, and until
            // then this is a launch like any other.
            phase: Phase::Starting,
            stalled: false,
            queued: outstanding,
            queued_messages: delivered_ahead
                .iter()
                .map(|pending| pending.preview(true))
                .chain(queue.iter().map(|pending| pending.preview(false)))
                .collect(),
            background: 0,
            tasks: Vec::new(),
            awaiting_background: false,
            turn_started_at: Some(iso8601(SystemTime::now())),
            question: None,
        }));
        let (tx, rx) = mpsc::unbounded_channel();
        self.residents.lock().expect("residents poisoned").insert(
            record.session_id.clone(),
            Handle {
                tx: tx.clone(),
                shared: shared.clone(),
            },
        );
        // Published here rather than left to the driver, for the reason
        // [`InjectionRegistry::adopt_all`] runs before the listener at all: the
        // first request after a restart must not be told that a session with a
        // turn in flight has nothing running. What the replay says is a round
        // trip to a shim away, and the composer would be open for all of it. So
        // the pessimistic half goes in now and the driver corrects it — the same
        // bargain [`InjectionRegistry::start_resident`] makes for a cold CLI.
        if record.turn_at.is_some() || outstanding > 0 {
            self.runs
                .lock()
                .expect("injections poisoned")
                .insert(self.key_of(Source::Claude, &record.session_id), running_turn(&record.session_id));
        }
        // A shim that has written nothing has nothing to replay, which is the
        // everyday shape of a resident spawned and restarted-past before it said
        // a word. Such a driver starts live, exactly like a fresh spawn.
        let replaying = link.total() > 0;
        // The recording this session already has, re-opened and appended to:
        // one file per session for its whole life, however many servers and
        // however many processes that life spans.
        let recorder = self.recorders.open(&record.session_id);
        recorder.attached(record.cli_pid, record.shim.as_deref(), record.turn_at, outstanding, link.total());
        let driver = Driver {
            registry: self.clone(),
            id: record.session_id.clone(),
            sidecars,
            link,
            record: record.run_id.clone(),
            recorder,
            live,
            recipe: None,
            tx,
            rx,
            shared,
            claim: Some(claim),
            queue,
            delivered_ahead,
            absorb: AbsorbWatch::default(),
            turn: None,
            ready: false,
            heard: Instant::now(),
            replaying,
            replay_open: false,
            replay_marked: false,
            // What the last server knew and the shim could not: whether a prompt
            // had gone in and not yet been answered.
            pending_turn: record.turn_at,
            idle_since: None,
            seq: 0,
            fatal: None,
            background: BackgroundTasks::default(),
            awaiting_background: false,
            awaiting_since: None,
            deaths: VecDeque::new(),
            rung: 0,
            recovering: false,
            retiring: false,
            limit_hint: limits::HintSlot::default(),
            ahead_noted: None,
            // A queue held after a limit stays held across the restart: the
            // alternative is a pump that feeds it straight to the account that
            // refused, and a second decision about the same limit.
            hold: record.hold.clone(),
            asked: Vec::new(),
            replay_asked: Vec::new(),
            replay_refusals: Vec::new(),
            replay_declines: Vec::new(),
            replay_gapped: false,
            // Empty, whatever the process is in the middle of: see the module
            // docs of `drafts` for why the replay does not rebuild it.
            draft: self.draft_writer(&record.session_id),
            phase_moved: std::sync::atomic::AtomicBool::new(false),
        };
        tracing::info!(
            session = %record.session_id,
            pid = record.cli_pid,
            shim = ?record.shim,
            turn = ?record.turn_at,
            queued = outstanding,
            steered = driver.delivered_ahead.len(),
            "re-attached to a resident claude that outlived the last server"
        );
        tokio::spawn(driver.run());
    }
}

/// The backlog an attach inherits, split by whether claude already has it.
struct Inherited {
    /// Written to the running process' stdin before the last server stopped,
    /// and still sitting in its buffer.
    delivered_ahead: VecDeque<Pending>,
    /// Never written anywhere.
    queue: VecDeque<Pending>,
}

/// The messages an adopted resident may still be given, out of the ones the
/// record says were waiting.
///
/// Every message this server did not accept itself arrives without an
/// environment — the journal cannot hold one — so the only process it can be
/// delivered to is the one already running. That makes the fingerprint the whole
/// test, and it is applied as a *prefix*: the queue is answered in order, so a
/// message the running process cannot take blocks everything behind it whether
/// it is dropped or kept, and keeping it would only mean a driver that can
/// neither deliver nor restart, waiting for ever.
///
/// # Why a steered message is not simply re-queued
///
/// An attach re-attaches to the *same process*, through the shim socket the
/// journal names. A message that was steered into it went into that process'
/// stdin, and stdin is the process': it survives this server restarting, and
/// the CLI will run it when the turn in front of it ends. Putting it back in
/// the queue would therefore deliver it twice — the exact failure
/// [`Driver::recover`] refuses for a turn that was in flight, for the same
/// reason: the side effects of a prompt run once are not undone by running it
/// again.
///
/// So it goes back where it was, and the only thing that ever re-delivers it is
/// the death of the process holding it — see [`Driver::requeue_delivered`],
/// which runs against a pipe that is provably closed.
///
/// # A `live` nobody wrote down
///
/// Then nothing is vetoed. These messages were accepted by the server that
/// started the process, against the fingerprint it knew and this one does not,
/// and re-judging them with *less* information than the decision was first made
/// with can only be wrong more often. The queue is kept whole and the first
/// message to bring a recipe says what the process is.
fn inherited_queue(session: &str, queued: &[runs::Queued], live: Option<&Fingerprint>) -> Inherited {
    let mut inherited = Inherited {
        delivered_ahead: VecDeque::new(),
        queue: VecDeque::new(),
    };
    for (index, message) in queued.iter().enumerate() {
        // …with one exception: a message that differs from the running process
        // in its login alone. That is what an account rotation leaves behind
        // when the server restarts while its continuation waits for background
        // work — the continuation and everything retargeted behind it name the
        // new account — and it is kept for the pump to rebuild a recipe for,
        // from the session's binding. One that does not match the binding is
        // failed out loud there, as before. See [`Driver::recipe_for_binding`].
        let kept_for_rebinding = live.is_some_and(|live| message.fingerprint.fits_but_for_login(live));
        if !message.fingerprint.accepted_by(live) && !kept_for_rebinding {
            tracing::warn!(
                session = %session,
                dropped = queued.len() - index,
                "a message waiting for settings this server cannot reproduce did not survive the restart"
            );
            break;
        }
        let pending = Pending {
            // A record from before the queue was addressable has no id, and a
            // message with no id is one no route could ever name. Minting one
            // keeps the message and gives it a handle.
            id: if message.id.is_empty() { new_uuid() } else { message.id.clone() },
            text: message.text.clone(),
            fingerprint: message.fingerprint.clone(),
            recipe: None,
            rotation_attempt: message.rotation_attempt,
            continuation: message.continuation.clone(),
        };
        if message.steered {
            inherited.delivered_ahead.push_back(pending);
        } else {
            inherited.queue.push_back(pending);
        }
    }
    inherited
}

/// One freshly started process and the two records that name it.
struct Spawned {
    link: Link,
    sidecars: Sidecars,
    /// The `runs.json` entry, so the driver can retire its own.
    record: String,
}

// ------------------------------------------------------------------ the turn

/// The turn in flight: one message, and everything it produces.
struct Turn {
    /// What the API reports as this session's injection.
    run: Arc<Run>,
    /// How far through claude's stdout this turn's prompt was written.
    ///
    /// The offset the journal records, kept here so that every rewrite of the
    /// record can restate it rather than having to remember whether it already
    /// had. See [`runs::RunRecord::turn_at`].
    at: u64,
    /// Distinguishes this turn from the next, so a ladder rung that comes due
    /// after it ended cannot signal the turn that replaced it.
    seq: u64,
    /// The rung the interrupt ladder is on; `None` until Stop is pressed.
    ladder: Option<u8>,
    /// Set for a turn opened for a message claude was handed early, until
    /// claude proves it opened a turn of its own for it.
    ///
    /// A turn this driver *wrote* is real by construction: the prompt went into
    /// stdin, so an answer is coming. A steered message is not — see
    /// [`Driver::settle_delivered`] — because the CLI may have folded it into
    /// the turn that was already running, in which case its `result` has
    /// already gone out and nothing else ever will. Cleared by the turn's own
    /// `system:init`; if that never comes, the settle closes the turn instead
    /// of leaving the session working on nothing for ever.
    provisional: bool,
    /// The [`Pending::rotation_attempt`] of the message this turn answers; `0`
    /// for a turn claude opened by itself.
    rotation_attempt: u8,
}

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
struct TaskSeen {
    id: String,
    tool_use_id: Option<String>,
    description: Option<String>,
    task_type: Option<String>,
}

impl TaskSeen {
    /// Read one task out of a `task_started` line or a snapshot entry.
    fn parse(value: &Value) -> Option<Self> {
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
struct TaskProgress {
    id: String,
    tool_use_id: Option<String>,
    subagent_type: Option<String>,
    last_tool_name: Option<String>,
    total_tokens: Option<u64>,
    tool_uses: Option<u64>,
    duration_ms: Option<u64>,
}

impl TaskProgress {
    /// Read one `task_progress` line, tolerating every field but the id.
    fn parse(value: &Value) -> Option<Self> {
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
struct BackgroundTasks {
    /// `(id, tool_use_id, description, task_type, first_seen)`, oldest first.
    entries: Vec<crate::sessions::BackgroundTask>,
}

impl BackgroundTasks {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn clear(&mut self) {
        self.entries.clear();
    }

    /// Record a task, or fill in what a later line said about a known one.
    ///
    /// Returns whether anything changed. `first_seen` is never rewritten: a
    /// snapshot republished every few seconds must not keep resetting the clock
    /// on work that has been running for half an hour.
    fn observe(&mut self, seen: TaskSeen) -> bool {
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
        self.entries.push(crate::sessions::BackgroundTask {
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
    fn progress(&mut self, progress: TaskProgress) -> bool {
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
    fn remove(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|task| task.id != id);
        before != self.entries.len()
    }

    /// Apply a whole-list snapshot: absorb what it names, drop what it does not.
    fn snapshot(&mut self, live: Vec<TaskSeen>) -> bool {
        let named: HashSet<&str> = live.iter().map(|task| task.id.as_str()).collect();
        let mut changed = self.entries.iter().any(|task| !named.contains(task.id.as_str()));
        self.entries.retain(|task| named.contains(task.id.as_str()));
        for seen in live {
            changed |= self.observe(seen);
        }
        changed
    }

    /// The first [`TASKS_ON_THE_WIRE`] of them, for the mirror.
    fn wire(&self) -> Vec<crate::sessions::BackgroundTask> {
        self.entries.iter().take(TASKS_ON_THE_WIRE).cloned().collect()
    }
}

// ---------------------------------------------------------------- the driver

/// The one task that owns a resident: its link, its queue and its state.
struct Driver {
    registry: Arc<InjectionRegistry>,
    /// The session id the transcript is under, which `system:init` may correct.
    id: String,
    sidecars: Sidecars,
    link: Link,
    /// This process' `runs.json` entry.
    record: String,
    /// The session's flight recording, or a handle that does nothing.
    ///
    /// Held by the driver rather than the registry because its lifetime *is*
    /// the resident's: opening it starts the task that follows the session's
    /// transcript files, and dropping it — which happens when this driver ends
    /// — is what stops that task after one last sweep. See [`crate::record`].
    recorder: Recorder,
    /// The settings the live process was started with, and what every message
    /// is compared against.
    ///
    /// `None` is "nobody wrote them down": a record from before the journal
    /// carried a fingerprint, adopted by a server new enough to look for one.
    /// Never a guess — see [`Fingerprint::accepted_by`] — and never for long:
    /// [`Driver::take_settings`] fills it in from the first message that brings
    /// a recipe and heals the record on disk at the same time, so an unknown
    /// survives at most one message rather than every restart from here on.
    live: Option<Fingerprint>,
    /// The whole recipe, for a process this server started.
    ///
    /// `None` after an attach. The journal records the settings but not the argv
    /// and emphatically not the environment — it holds an account's token — so a
    /// resident inherited from a previous server cannot be respawned from what is
    /// on disk. The first message to arrive fixes that: a message whose
    /// fingerprint fits the running process describes a process equivalent to it,
    /// which is precisely what a respawn needs. Until then the recovery ladder
    /// has nothing to spawn and says so.
    recipe: Option<Recipe>,
    /// A clone of the driver's own inbox, for the ladder's timers.
    tx: mpsc::UnboundedSender<Cmd>,
    rx: mpsc::UnboundedReceiver<Cmd>,
    shared: Arc<Mutex<ResidentState>>,
    /// Released when the process is gone, and not before.
    claim: Option<lock::InjectLock>,
    queue: VecDeque<Pending>,
    /// Messages already written to claude's stdin whose turn has not opened yet.
    ///
    /// The whole of steering, in one field. claude 2.1.231 under stream-json
    /// *buffers* a `user` line written during a turn and runs it the moment
    /// that turn's `result` goes out — verified against the real CLI, which
    /// answers each buffered message with its own `system:init` and its own
    /// `result` — so writing one early is a way to put it at the front of the
    /// queue without interrupting anything.
    ///
    /// What it costs is a second queue to account for, because between the
    /// write and the turn the message is in neither place the old code looked:
    /// out of [`Driver::queue`], not yet in [`Driver::turn`]. Everything about
    /// these entries follows from where they physically are:
    ///
    /// * they run **before** [`Driver::queue`], because they are already in the
    ///   pipe and the CLI reads it in order — see [`Driver::pump`];
    /// * their turn opens **without another write**, or the prompt would run
    ///   twice;
    /// * they cannot be deleted, and a Stop does not stop them: the CLI has
    ///   them and nothing in this server can reach into its stdin buffer;
    /// * a process that dies takes them nowhere — a closed pipe was never read
    ///   — so they go back to the head of the queue and are delivered to the
    ///   replacement. See [`Driver::requeue_delivered`].
    ///
    /// And they leave it **early** when claude folds one into the turn that is
    /// already running, which current CLIs do at the next tool boundary: the
    /// transcript then holds it, and nothing is left to wait for. See
    /// [`Driver::absorb`].
    delivered_ahead: VecDeque<Pending>,
    /// Where the driver has read the transcript up to, looking for those folds.
    absorb: AbsorbWatch,
    turn: Option<Turn>,
    /// False until `system:init`. Nothing may be written before it.
    ready: bool,
    /// When claude last said anything at all, for the stall watchdog.
    heard: Instant,
    /// Whether what the link is handing out is still the session's history.
    ///
    /// True only for a driver built by [`InjectionRegistry::attached`], and only
    /// until the replay reaches the offset the shim reported. Nothing may act
    /// while it holds — see [`Driver::replay`].
    replaying: bool,
    /// Whether the history replayed so far ends in the middle of a turn.
    ///
    /// Only meaningful while `replaying` is; settled and cleared by
    /// [`Driver::settle_replay`].
    replay_open: bool,
    /// Whether the replay has walked past the journal's turn mark without yet
    /// meeting that turn's `result` — the window in which an `init` is the
    /// turn's own first output rather than a fresh process' preamble ending.
    replay_marked: bool,
    /// The stdout offset the journal says a turn was written in at.
    ///
    /// Only ever set by [`InjectionRegistry::attached`], and only until the
    /// replay walks past it. It is the half of the session's state that claude's
    /// stdout cannot carry — see [`runs::RunRecord::turn_at`] — and it is
    /// consumed exactly like a line of the replay, in its place in the stream,
    /// so that a `result` recorded after it still ends the turn it opened.
    pending_turn: Option<u64>,
    /// When this resident last had nothing at all to do, for the idle shutdown.
    ///
    /// `None` whenever it does have something to do, so the clock starts from
    /// the moment the session went quiet rather than from the last tick.
    idle_since: Option<Instant>,
    seq: u64,
    /// Set when the driver is ending for a reason the tombstone cannot explain
    /// — a respawn whose replacement would not start, or a recovery ladder that
    /// ran out of rungs.
    fatal: Option<String>,
    /// The background tasks claude has started and not yet reported finished.
    ///
    /// Keyed by id rather than counted, because the authoritative event —
    /// `system:background_tasks_changed` — carries the whole live list every
    /// time it changes, so the honest thing to do with it is to replace the map.
    /// That also makes the whole mechanism replay-safe for free: re-running the
    /// history converges on the last list claude published, however many
    /// `task_started` lines went past on the way.
    background: BackgroundTasks,
    /// Set when the head of the queue is waiting for background work to finish
    /// before its settings change may restart the process.
    awaiting_background: bool,
    /// When that wait began, for the ceiling on it.
    ///
    /// Separate from the flag because the flag is set and cleared on every pass
    /// through the pump — the watchdog re-pumps a parked driver once a minute —
    /// and a clock restarted by each of those would never run out.
    awaiting_since: Option<Instant>,
    /// When this resident's process died, for the crash-loop breaker.
    deaths: VecDeque<Instant>,
    /// Which rung of the recovery ladder the next respawn is on: 0 = resume,
    /// 1 = fork. Reset once a replacement gets as far as `system:init`.
    rung: u8,
    /// True from a recovery respawn until its replacement says `init`: the
    /// window in which a death with no turn and no queue is still the
    /// ladder's business rather than an idle process ending.
    recovering: bool,
    /// Set when this server is ending the process on purpose and an exit is
    /// therefore not a crash — today only the idle shutdown.
    retiring: bool,
    /// A refused weekly window of unknown scope, waiting a few seconds for the
    /// `result` that says whose it was. The limit log's, and nothing else's;
    /// see [`limits::WindowHint`].
    limit_hint: limits::HintSlot,
    /// The near window a move ahead of a limit last found nowhere to go for,
    /// as `account window reset`: that outcome is logged once per window
    /// crossing rather than at every quiet point. See [`Driver::rotate_ahead`].
    ahead_noted: Option<String>,
    /// Set when a limit ended this session's turn and no account could take
    /// the session over: nothing more goes to the exhausted account until the
    /// limit lifts or somebody acts. See [`Driver::rotate_after_limit`].
    hold: Option<Hold>,
    /// The questions claude is blocked on, oldest first: in practice one, since
    /// the turn stops at the first. While any is here the session is waiting
    /// on the user and nothing may end the process or the turn from under it —
    /// see [`Driver::ask`].
    asked: Vec<OpenQuestion>,
    /// What the history replayed so far leaves open: questions nobody has
    /// answered, and permission prompts nobody has refused yet. Only meaningful
    /// while `replaying` is; settled by [`Driver::settle_questions`].
    replay_asked: Vec<OpenQuestion>,
    replay_refusals: Vec<PermissionRequest>,
    /// The other control requests the history leaves unanswered, by id: each
    /// is declined again when the replay ends, as a refusal is.
    replay_declines: Vec<String>,
    /// Whether the history replayed so far lost bytes to the shim's ring. A
    /// question asked inside the lost stretch is invisible to the replay, and
    /// only the record still knows claude is waiting on it — see
    /// [`Driver::recall_questions_lost_in_a_gap`].
    replay_gapped: bool,
    /// The count of the think claude is in the middle of, which the
    /// transcript cannot say, for the session feed's activity bar. See
    /// [`super::drafts`].
    draft: super::drafts::DraftWriter,
    /// Set by [`Driver::set_phase`] when the phase moved, and taken by the
    /// next [`Driver::publish`], which pokes the feed again: see `set_phase`.
    /// Atomic only because the driver is shared by reference across awaits.
    phase_moved: std::sync::atomic::AtomicBool,
}

/// A question claude is blocked on, as the driver holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenQuestion {
    /// The CLI's id for the request; the answer has to name it.
    request_id: String,
    /// The `tool_use` it is, which is how its answer is seen on stdout.
    tool_use_id: Option<String>,
    /// The tool input's `questions`, verbatim: the answer echoes it.
    questions: Value,
}

impl OpenQuestion {
    fn of(request: &PermissionRequest) -> OpenQuestion {
        OpenQuestion {
            request_id: request.request_id.clone(),
            tool_use_id: request.tool_use_id.clone(),
            questions: request.questions(),
        }
    }
}

/// How far the driver has read the session's transcript for steered messages
/// claude took into the turn in flight. See [`Driver::absorb`].
#[derive(Debug, Default)]
struct AbsorbWatch {
    /// Where reading starts, or `None` while no steered message is outstanding.
    anchor: Option<AbsorbAnchor>,
    /// Whether a poll is already on its way, so a steer does not start a second.
    armed: bool,
    /// Whether something was taken out of the queue since the session's feed
    /// was last poked about it. See [`Driver::on_absorb`].
    unannounced: bool,
}

#[derive(Debug)]
struct AbsorbAnchor {
    /// The session id the transcript was looked up under. An `init` that moves
    /// the session — a fork — moves the file, and a fork's file opens with a
    /// copy of the history, folds and all: so the anchor is dropped and taken
    /// again at the new file's end rather than read from its start.
    session: String,
    /// The transcript, once it exists.
    file: Option<PathBuf>,
    /// The byte offset everything before which has been read, or predates the
    /// first steer it is watching for.
    offset: u64,
}

/// What a fold read out of the transcript could be: the prompt as it went in,
/// and as the CLI wrote it back.
///
/// Whitespace at the ends and line endings are the only differences allowed:
/// the CLI records the very text it was handed, and anything looser would let a
/// short message — "继续" — match a fold of some other message.
fn same_prompt(written: &str, recorded: &str) -> bool {
    let normal = |text: &str| text.trim().replace("\r\n", "\n");
    normal(written) == normal(recorded)
}

/// The whole lines appended to `path` past `offset`, and the offset after them.
///
/// A line still being written is left for the next read, and a file that is
/// now shorter than `offset` — replaced, not appended to — is read from its new
/// end on: whatever it says now cannot be told apart from what was read before.
/// `needle` filters lines before they are parsed, so megabytes of tool output
/// cost a byte scan and not a JSON parse.
async fn appended_lines(path: &Path, offset: u64, needle: &[u8]) -> (Vec<Value>, u64) {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return (Vec::new(), offset);
    };
    let Ok(len) = file.metadata().await.map(|meta| meta.len()) else {
        return (Vec::new(), offset);
    };
    if len < offset {
        return (Vec::new(), len);
    }
    if len == offset || file.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
        return (Vec::new(), offset);
    }
    let mut bytes = Vec::with_capacity((len - offset) as usize);
    if (&mut file).take(len - offset).read_to_end(&mut bytes).await.is_err() {
        return (Vec::new(), offset);
    }
    let Some(end) = memchr::memrchr(b'\n', &bytes) else {
        return (Vec::new(), offset);
    };
    let finder = memchr::memmem::Finder::new(needle);
    let lines = bytes[..end]
        .split(|byte| *byte == b'\n')
        .filter(|line| finder.find(line).is_some())
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .collect();
    (lines, offset + end as u64 + 1)
}

/// A queue held after a limit with nowhere to go.
///
/// The alternative, which is what every release before this one did, fed the
/// rest of the queue to the account that had just refused — one real session
/// shows five rejections inside a minute. While it holds, a further refusal
/// from the same account is the same limit and decides nothing new, and the
/// choice is asked again every watchdog tick and whenever an account frees up
/// ([`Driver::recheck_hold`]). What lets it go:
///
/// * an account to move to appearing — the switch the first decision could
///   not make;
/// * the limit lifting — its rest ending or being cleared — which puts a
///   continuation for the failed message at the head of the queue first;
/// * a new message, which is somebody acting — and which, when it was sent
///   under another account, takes the held messages with it;
/// * a steer, which says "now";
/// * a new process under another account, for whatever reason it started.
///
/// Journalled on the run record, so a server that restarts mid-hold keeps
/// holding — see [`runs::RunRecord::hold`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Hold {
    /// The account that refused.
    account: String,
    /// Its display name, for the continuation that follows the lift.
    label: String,
    /// The limit, as the rest and the decision name it.
    class: LimitClass,
    scope: String,
    /// When it lifts, epoch milliseconds; `None` until somebody acts or the
    /// rest is cleared.
    until_ms: Option<i64>,
    /// How many switches the failed message's chain has had, for the cap.
    switches: u8,
    /// Whether the failed message still needs a continuation — cleared once
    /// one is queued, or when somebody's new message takes over.
    continue_failed: bool,
}

/// A limit that triggered a rotation, worded once for everything that reports
/// it: the log, the notification and the continuation.
#[derive(Debug, Clone)]
struct LimitHit {
    class: LimitClass,
    scope: String,
    /// When it lifts, epoch milliseconds, when anything said.
    until_ms: Option<i64>,
    /// [`rotation::reason`].
    reason: String,
    /// [`rotation::until_text`], in this server's zone.
    until: Option<String>,
}

impl LimitHit {
    fn new(class: LimitClass, scope: &str, until_ms: Option<i64>) -> LimitHit {
        LimitHit {
            class,
            scope: scope.to_string(),
            until_ms,
            reason: rotation::reason(class, scope),
            until: rotation::until_text_local(until_ms),
        }
    }
}

/// [`rotation::Plan`] with the account it names resolved to a name.
enum Choice {
    Switch(AccountRef),
    Nowhere(RotationOutcome),
}

impl Driver {
    async fn run(mut self) {
        let mut watchdog = tokio::time::interval(self.registry.watchdog_interval);
        watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        watchdog.tick().await; // the first tick is immediate

        // The message a fresh spawn was born holding goes in before anything is
        // heard: claude says nothing — not even `init` — until it has read a
        // first message (see `pump`), so an event-driven first delivery would
        // wait for ever on the event this delivery unlocks. A no-op for an
        // attached driver, whose replay gate holds until history has caught up.
        self.pump().await;
        // Steered messages inherited from the last server are watched for
        // folds like any others, from here on.
        self.arm_absorb();

        loop {
            // The two receivers are borrowed by hand: `select!` needs disjoint
            // borrows, and a method on `self` would take the whole of it.
            let wake = {
                let Driver { link, rx, .. } = &mut self;
                tokio::select! {
                    output = link.recv() => Wake::Output(output),
                    command = rx.recv() => Wake::Command(command),
                    _ = watchdog.tick() => Wake::Tick,
                }
            };
            match wake {
                Wake::Output(Some(Output::Line(line))) => {
                    // Recorded before it is acted on, and whether or not it is
                    // history: a recording is what claude *said*, in order, and
                    // the replay flag is the one thing a reader cannot work out
                    // for itself afterwards.
                    self.recorder.stdout(&line, self.replaying);
                    if self.replaying {
                        self.replay(&line).await;
                        self.settle_replay().await;
                    } else {
                        self.on_line(&line).await;
                    }
                }
                Wake::Output(Some(Output::Gap { from, to })) => {
                    // Lost output is not lost work: the transcript is the record
                    // and the feed reads it directly. What it does mean is that
                    // a `result` may have been in the gap, so the turn is left
                    // for the process' own exit or the user's Stop to end.
                    tracing::warn!(session = %self.id, from, to, "the session shim's replay buffer overflowed");
                    self.recorder.gap(from, to);
                    self.touch();
                    if self.replaying {
                        // A gap in the *history* is the one thing that can leave
                        // the reconstruction genuinely uncertain, so it is
                        // resolved the safe way: something was lost, and what it
                        // was may have been a turn. The journal's mark is taken
                        // rather than left pending, because whatever it said is
                        // already covered by that, and leaving it would let it
                        // re-open a turn the lines after the gap had ended.
                        self.cross_turn_mark();
                        self.replay_open = true;
                        self.replay_gapped = true;
                        self.settle_replay().await;
                    }
                }
                Wake::Output(None) => {
                    // A credential refused hard enough to end the process is
                    // written down first; the ladder below is unchanged by it.
                    self.note_exit_limit().await;
                    // The link is gone. Either a replacement takes its place and
                    // the session goes on, or there is nothing left to run.
                    if self.recover().await {
                        continue;
                    }
                    break;
                }
                Wake::Command(Some(command)) => self.on_command(command).await,
                // Every sender is gone, which cannot happen while the map holds
                // one; treat it as a shutdown rather than spinning on it.
                Wake::Command(None) => break,
                Wake::Tick => self.watchdog().await,
            }
        }
        self.finish().await;
    }

    // -------------------------------------------------------------- replay

    /// Read one line of history, for the state it implies and nothing else.
    ///
    /// Everything a re-attached driver knows about the session it inherited
    /// comes from here: the shim replays claude's whole stdout from byte zero,
    /// and this walks it. Nothing may act — a `result` in the replay is a turn
    /// that ended before this server existed, and notifying a phone about it, or
    /// writing the next queued message into the middle of a live turn on the
    /// strength of it, is the failure mode this split exists to prevent.
    ///
    /// The one exception is the session id. `system:init` is authoritative about
    /// which transcript claude is writing, and a fork that happened while this
    /// server was down is something the replay is the *only* record of.
    async fn replay(&mut self, line: &str) {
        self.cross_turn_mark();
        match classify(line) {
            Event::Init { session, model } => {
                self.ready = true;
                // An `init` closes the start-up *preamble* — the hook lines and
                // junk above it must not read as an open turn. It does not close
                // a turn whose journal mark it comes after: claude says `init`
                // only once it has read a first message, so an init past the
                // mark is the answer's first line, not evidence the turn ended.
                self.replay_open = self.replay_marked;
                self.canonicalize_model(model);
                if let Some(id) = session {
                    if id != self.id {
                        self.adopt(&id).await;
                    }
                }
            }
            Event::Result { .. } => {
                self.replay_open = false;
                self.replay_marked = false;
                // A turn cannot end with a question open — it blocks until it is
                // answered — so whatever the history had asked was settled
                // before this line, by whoever was listening then.
                self.replay_asked.clear();
                self.replay_refusals.clear();
                self.replay_declines.clear();
            }
            // Mid-turn by definition: claude stops to ask, and the turn waits.
            // Remembered rather than acted on, like everything in a replay —
            // whether it is still waiting is the rest of the history's to say,
            // and [`Driver::settle_questions`] acts on what is left.
            Event::Permission(request) => {
                self.replay_open = true;
                if request.is_question() {
                    self.replay_asked.push(OpenQuestion::of(&request));
                } else {
                    self.replay_refusals.push(*request);
                }
            }
            Event::ControlCancel { request_id } => {
                self.replay_asked.retain(|question| question.request_id != request_id);
                self.replay_refusals.retain(|request| request.request_id != request_id);
                self.replay_declines.retain(|declined| *declined != request_id);
            }
            // Declined again at the boundary, like a refusal: a request nobody
            // answered is a CLI waiting for ever, and answering twice is free.
            Event::ControlRequest { request_id } => {
                self.replay_open = true;
                self.replay_declines.extend(request_id);
            }
            // claude echoes the result it built from an answer — or a refusal —
            // as a `user` line, which is how the history says the request it
            // answers is over.
            Event::Speech => {
                self.replay_open = true;
                if !self.replay_asked.is_empty() || !self.replay_refusals.is_empty() {
                    let answered = answered_in(line);
                    let settled = |tool_use: &Option<String>| tool_use.as_ref().is_some_and(|id| answered.contains(id));
                    self.replay_asked.retain(|question| !settled(&question.tool_use_id));
                    self.replay_refusals.retain(|request| !settled(&request.tool_use_id));
                }
            }
            Event::BackgroundTasks { tasks } => {
                self.background.snapshot(tasks);
            }
            Event::TaskStarted { task } => {
                self.background.observe(task);
            }
            // Reconstructed like the rest of the list, and — like the rest of the
            // background events — not work: a progress line is published while
            // the session sits idle, so counting it would leave every session
            // that ever ran a background task replaying as mid-turn.
            Event::TaskProgress { progress } => {
                self.background.progress(progress);
            }
            Event::TaskFinished { id } => {
                self.background.remove(&id);
            }
            // Not work, and this is the arm that says so. The CLI publishes
            // `system` lines while a session sits idle — `commands_changed` when
            // a hook edits the command list, `task_updated` for a background
            // task — and one of those after the last `result` used to leave the
            // whole replay reading as mid-turn. The live path has always refused
            // to open a turn on one; this is the replay agreeing.
            Event::Housekeeping => {}
            // Anything else claude wrote — an assistant message, a tool result,
            // a thinking-token count, a line that is not JSON — is work, and
            // work after the last `result` is a turn that had not ended.
            //
            // The background events above are pointedly *not* work: on 2.1.231
            // a task finishing while the session is idle writes
            // `task_updated` and `task_notification` with no turn around them,
            // and counting those would leave every session that ever ran a
            // background task looking permanently busy.
            //
            // [`Event::Speech`] is read the same way, in an arm of its own only
            // because it can also settle a question: a replay must reconstruct,
            // never open. The turn it would have opened live is the same turn
            // the `replay_open` below reconstructs at the boundary, and erring
            // towards "in flight" is the whole policy of this function — see
            // [`Driver::settle_replay`].
            //
            // A `rate_limit_event` is here because it was [`Event::Other`]
            // until the limit log and the quota readings wanted it told apart,
            // and the replay reads it exactly as it always did. It is neither
            // recorded nor filed: a replay reconstructs, a limit hit before
            // this server existed was recorded by the one that saw it or is
            // history, and a reading replayed now would be stamped with the
            // wrong time.
            Event::ControlResponse | Event::Other | Event::RateLimit { .. } | Event::UsageReading { .. } | Event::Unparseable => {
                self.replay_open = true
            }
        }
    }

    /// Take the journal's turn mark, once the replay has reached where it sits.
    ///
    /// Called before each replayed item, and compared against the offset *after*
    /// that item: an item ending at or before the mark was written before the
    /// prompt went in and says nothing about it, and one ending after it came
    /// back from claude in answer. So a `result` from an earlier turn cannot
    /// clear this turn, and a `result` from this one still does — which is the
    /// whole reason the journal records a place rather than a flag.
    fn cross_turn_mark(&mut self) {
        let Some(at) = self.pending_turn else { return };
        if self.link.at() <= at {
            return;
        }
        self.pending_turn = None;
        self.replay_open = true;
        // Remembered past the conversion, because the turn's own `init` may
        // still be ahead: everything from here to the next `result` is the
        // marked turn, whatever shape its lines take.
        self.replay_marked = true;
    }

    /// Stop replaying once the history has run out, and act on what it said.
    ///
    /// The boundary is a byte offset and has to be exact: the shim says how far
    /// claude had written when the link was made, and the line that ends there
    /// is the last one that already happened. Deciding here rather than when the
    /// *next* line arrives is what makes a session that has been sitting quietly
    /// since the restart come back as anything at all — the next line may be
    /// hours away, or may never come.
    ///
    /// Erring towards "a turn is in flight" is deliberate. Reading a live turn
    /// as idle unblocks the composer and lets the next message be written into
    /// the middle of it — two prompts in one turn, which is the interleaving
    /// this whole mechanism exists to stop. Reading an idle session as working
    /// costs a run that shows as running until the next `result`, the process'
    /// own exit, or the user pressing Stop.
    async fn settle_replay(&mut self) {
        if self.link.at() < self.link.total() {
            return;
        }
        self.replaying = false;
        self.heard = Instant::now();
        self.settle_background().await;
        // A mark still pending here sits past everything claude has written,
        // which is the plainest case there is: a prompt went in and the process
        // has not said a word since. It is also the one the stdout alone can
        // never show, and the reason this record exists.
        if self.pending_turn.take().is_some() {
            self.replay_open = true;
        }
        self.replay_marked = false;
        let mut open = std::mem::take(&mut self.replay_open);
        if std::mem::take(&mut self.replay_gapped) {
            self.recall_questions_lost_in_a_gap();
        }
        // A question the history leaves open is a turn in flight by definition:
        // claude is blocked on it, mid-turn, for as long as it takes. It is also
        // the one case the transcript must not be asked about, because the
        // transcript reads a tail ending on `AskUserQuestion` as a session
        // waiting for input — which is true, and would let go of the very turn
        // the question belongs to.
        let asking = !self.replay_asked.is_empty();
        open |= asking;
        // The reconstruction above is stdout's account of the session, and
        // stdout is not the only witness. Before a turn *nobody can see the
        // start of* is opened — and one opened here stays open for as long as
        // claude stays quiet, which for a resident that is between turns is for
        // ever — the transcript is asked whether it agrees. See
        // [`Driver::transcript_is_between_turns`] for why it is worth asking and
        // when it declines to answer.
        if open && !asking && self.transcript_is_between_turns().await {
            tracing::warn!(
                session = %self.id,
                "the replay ended mid-turn but the transcript says the session is between turns; not opening one"
            );
            self.recorder.lifecycle(
                Stage::Settle,
                Some("the replay read as mid-turn and the transcript disagreed; no turn opened"),
            );
            open = false;
        }
        if open {
            tracing::info!(session = %self.id, "the resident's replay ended mid-turn; assuming it is still working");
            self.turn = Some(Turn {
                // The run adoption published, where there is one: the turn the
                // replay just reconstructed *is* the one it was published for,
                // and opening a second would leave the first running for ever.
                run: self.launching_turn(),
                // Re-marked at the boundary rather than left where it was: from
                // here on everything replayed is history and the next `result`
                // is this turn's. A second restart therefore reconstructs the
                // same turn from the same evidence instead of re-reading the
                // work that opened it.
                at: self.link.at(),
                seq: self.seq,
                ladder: None,
                provisional: false,
                rotation_attempt: 0,
            });
            self.set_phase(Phase::Working);
        } else {
            tracing::info!(
                session = %self.id,
                background = self.background.len(),
                "the resident's replay ended between turns"
            );
            // Nothing was in flight and nothing is waiting, so the run adoption
            // published on the pessimistic side has to come back off: leaving it
            // would block the composer on a turn that ended before this server
            // started.
            if self.queue.is_empty() {
                self.withdraw_published_run();
            }
        }
        // Either way the record is restated: a turn the journal remembered and
        // the replay showed ending has to stop being remembered, or a session
        // that has been idle since the restart comes back busy after the next one.
        let at = self.turn.as_ref().map(|turn| turn.at);
        self.note_progress(at);
        self.settle_questions().await;
        self.publish();
        self.pump().await;
    }

    /// Take the record's word for the questions a gap in the history hid.
    ///
    /// A question asked inside the stretch the shim's ring dropped never reaches
    /// the replay, and a CLI blocked on it writes nothing more of its turn — so
    /// the history ends looking like a turn that is over, the transcript (whose
    /// tail is the `AskUserQuestion` call) agrees, and without this the question
    /// would be expired, no turn opened, and the next message written into a
    /// CLI that is waiting for an answer it would then never get. A question
    /// still `pending` on record was answered by nobody — every answer moves the
    /// record before it is written, and so does a stop — so here the record is
    /// the better witness: it is held open again, oldest first, ahead of
    /// whatever the history after the gap did show.
    fn recall_questions_lost_in_a_gap(&mut self) {
        let mut recalled: Vec<OpenQuestion> = self
            .registry
            .questions
            .pending_for(&self.id)
            .into_iter()
            .filter(|stored| !self.replay_asked.iter().any(|question| question.request_id == stored.request_id))
            .map(|stored| OpenQuestion {
                request_id: stored.request_id,
                tool_use_id: stored.tool_use_id,
                questions: stored.questions,
            })
            .collect();
        if recalled.is_empty() {
            return;
        }
        tracing::warn!(
            session = %self.id,
            questions = recalled.len(),
            "the history lost a stretch to the shim's ring; holding the questions still pending on record open"
        );
        self.recorder.lifecycle(
            Stage::Settle,
            Some(&format!(
                "{} question(s) pending on record, unseen across a replay gap: held open",
                recalled.len()
            )),
        );
        recalled.append(&mut self.replay_asked);
        self.replay_asked = recalled;
    }

    /// Act on what the history left open: the questions claude is still
    /// blocked on, and the permission prompts nobody refused yet.
    ///
    /// The record and the history are two witnesses, and each knows something
    /// the other cannot. stdout says whether claude is *still waiting* — an
    /// answer it has taken is echoed as a result, a withdrawn request as a
    /// cancel. The record says what the user *did*, which never reaches stdout
    /// until claude acts on it. So:
    ///
    /// * a question waiting in both is held open again, as if this server had
    ///   heard it asked — without a second notification;
    /// * a question claude is waiting on that the record says was answered is
    ///   one whose answer never got written, because the last server died
    ///   between the compare-and-set and the write. The answer is written again:
    ///   claude ignores a response it has already had, so this is safe even when
    ///   the first one did get through and its echo is merely still on its way;
    /// * an expired one — expired by a boot that could not yet see the process
    ///   was alive — is taken back to pending, unless somebody already answered
    ///   it as a message, in which case claude is told so and its turn goes on;
    /// * a question the record has *cancelled* — a stop — and claude is still
    ///   waiting on is a stop whose interrupt never reached claude: the last
    ///   server died between moving the record and writing the line. The stop is
    ///   made again, so the turn ends as the user asked rather than carrying on
    ///   from a dismissal;
    /// * a question the record has pending and the history has not is over,
    ///   whatever ended it, and is expired — unless the history lost a stretch
    ///   to the shim's ring, which [`Driver::recall_questions_lost_in_a_gap`]
    ///   has already answered for.
    ///
    /// Refusals and declined requests are simply sent again. Doing it twice
    /// costs nothing, and a request left unanswered is a turn blocked for ever.
    async fn settle_questions(&mut self) {
        for request in std::mem::take(&mut self.replay_refusals) {
            self.refuse(&request).await;
        }
        for request_id in std::mem::take(&mut self.replay_declines) {
            self.decline_request(Some(request_id)).await;
        }
        let mut stop_again = false;
        let waiting = std::mem::take(&mut self.replay_asked);
        let store = self.registry.questions.clone();
        let now = questions::now_ms();
        for stale in store.pending_for(&self.id) {
            if !waiting.iter().any(|question| question.request_id == stale.request_id) {
                store.close(&stale.request_id, QuestionState::Expired, now);
            }
        }
        for question in waiting {
            let stored = store.get(&question.request_id);
            let state = stored.as_ref().map(|stored| stored.state);
            let reopened =
                state == Some(QuestionState::Expired) && store.rewind(&question.request_id, QuestionState::Expired, QuestionState::Pending, now);
            match (state, reopened) {
                (None, _) | (Some(QuestionState::Pending), _) | (Some(QuestionState::Expired), true) => {
                    tracing::info!(session = %self.id, request = %question.request_id, "claude is still waiting on a question it asked before the restart");
                    self.hold_question(question);
                }
                (Some(QuestionState::Cancelled), _) => {
                    tracing::info!(session = %self.id, request = %question.request_id, "a stop never reached claude before the restart; stopping the turn again");
                    stop_again = true;
                }
                _ => {
                    // Answered or dismissed while claude still waits — or
                    // expired and answered as a message in the moment between
                    // the two reads above. The stored response if there
                    // is one, and otherwise a refusal that says why.
                    let stored = store.get(&question.request_id);
                    let response = match stored.as_ref() {
                        Some(Stored {
                            delivery: Some(delivery), ..
                        }) => delivery.clone(),
                        Some(Stored {
                            state: QuestionState::Answered,
                            ..
                        }) => questions::deny(questions::ANSWERED_ELSEWHERE),
                        _ => questions::deny(questions::DISMISSED),
                    };
                    tracing::info!(session = %self.id, request = %question.request_id, "writing a question's settled answer again after the restart");
                    let line = questions::control_response(&question.request_id, &response);
                    self.recorder.lifecycle(
                        Stage::Question,
                        Some(&format!("{} was settled before the restart; telling claude again", question.request_id)),
                    );
                    self.recorder.stdin(&line);
                    if let Err(error) = self.link.send_line(&line).await {
                        tracing::warn!(session = %self.id, %error, "cannot write a settled answer to claude");
                    }
                }
            }
        }
        if stop_again {
            self.recorder.lifecycle(
                Stage::Interrupt,
                Some("a question was stopped before the restart and claude is still waiting on it"),
            );
            self.interrupt().await;
        }
        self.announce();
    }

    /// Whether claude's own transcript says this session is *not* mid-turn.
    ///
    /// The independent witness, and the answer to the one question this driver
    /// cannot ask the stream. Everything a re-attached driver knows about an
    /// inherited turn is inferred from stdout, and stdout cannot say what it
    /// never wrote: a `system` line published while the session sat idle, a
    /// replay gap, or a turn mark an earlier boot left behind all read as "still
    /// working", and nothing in the stream ever contradicts them — the `result`
    /// that would have said otherwise belongs to a turn that ended before this
    /// server existed. A turn reconstructed from one of those never ends, and
    /// the queue behind it never moves. That is the ghost this asks about.
    ///
    /// The transcript is written by claude itself and is already trusted for
    /// exactly this question elsewhere: [`super::InjectionRegistry::was_cut_short`]
    /// reads it to decide whether a run that outlived its server was cut short.
    /// Reusing [`crate::sessions::claude::claude_activity_status`] keeps one
    /// definition of mid-turn rather than two that can drift — a tail ending on
    /// the assistant's answer is a turn that finished, and one ending on a
    /// `tool_use` or a person speaking is a turn that did not.
    ///
    /// Three ways it declines to answer, and every one of them errs the same
    /// way — towards "a turn is in flight" — because unblocking the composer on
    /// a live turn is the expensive mistake and a ghost is the cheap one:
    ///
    /// * **no transcript at all**: a session whose first turn has yet to write a
    ///   line, which is precisely a turn in flight;
    /// * **written within [`TRANSCRIPT_SETTLES_AFTER`]**: the file may be in the
    ///   middle of recording the very prompt being asked about;
    /// * **a scan that runs out of budget**, which `claude_activity_status`
    ///   already reports as `Running` for the same reason.
    async fn transcript_is_between_turns(&self) -> bool {
        let Some(path) = crate::sessions::claude::find_claude_session_file(&self.registry.paths, &self.id).await else {
            return false;
        };
        let settled = tokio::fs::metadata(&path)
            .await
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= TRANSCRIPT_SETTLES_AFTER);
        if !settled {
            return false;
        }
        crate::sessions::claude::claude_activity_status(&path).await == SessionStatus::Waiting
    }

    /// Let go of a turn claude's transcript says is not happening.
    ///
    /// Withdrawn rather than failed, for the reason
    /// [`Driver::withdraw_published_run`] gives: nothing ran, so there is
    /// nothing to report, and a failure raised on every restart would be noise
    /// about something that went fine. What matters is the queue — parked on
    /// this turn since the attach, and released by the pump at the end.
    async fn discard_ghost_turn(&mut self) {
        let Some(turn) = self.turn.take() else { return };
        tracing::warn!(
            session = %self.id,
            quiet_for = ?self.heard.elapsed(),
            "nothing has been heard for a long time and the transcript says this session is between turns; \
             letting go of a turn that is not happening"
        );
        self.recorder.lifecycle(
            Stage::Settle,
            Some("no output and a transcript between turns; the reconstructed turn was let go"),
        );
        // Only if it is still the run this turn was published for: `pump` below
        // may want it back, and a run some other path has since put there is not
        // this driver's to take away.
        if self.published_run().is_some_and(|published| Arc::ptr_eq(&published, &turn.run)) {
            self.withdraw_published_run();
        }
        // The mark goes with it, or the next boot reconstructs the same ghost
        // from the same record — which is how one of these survives a restart.
        self.note_progress(None);
        self.set_phase(Phase::Idle);
        self.publish();
        self.pump().await;
    }

    // -------------------------------------------------------------- stdout

    /// Dispatch one line of claude's stdout.
    ///
    /// Tolerant by policy: an unparseable line is logged and stepped over,
    /// never treated as an error and never allowed to end a turn. The stream is
    /// versioned by a CLI that adds types and fields between patch releases,
    /// and a parser that refused what it did not recognise would turn each of
    /// those into a session that silently stops answering.
    async fn on_line(&mut self, line: &str) {
        self.touch();
        // Parsed once for both of its readers. The draft keeps the count of a
        // think in flight, which the transcript will never say, and `classify`
        // decides what the driver does about the line; neither is a reason to
        // read a large tool result twice.
        let value = serde_json::from_str::<Value>(line).ok();
        if let Some(value) = &value {
            self.draft.observe(value);
        }
        match value.as_ref().map_or(Event::Unparseable, classify_value) {
            Event::Init { session, model } => {
                self.canonicalize_model(model);
                self.on_init(session.as_deref()).await
            }
            Event::Result {
                is_error,
                subtype,
                detail,
                api_status,
            } => self.on_result(is_error, &subtype, detail.as_deref(), api_status).await,
            // Recorded and nothing else: the turn it precedes runs, or fails,
            // exactly as it would have. The refusal's windows are a reading
            // like any other.
            Event::RateLimit { info } => {
                self.note_usage(&info).await;
                self.note_rate_limit(&info).await
            }
            Event::UsageReading { info } => self.note_usage(&info).await,
            // Acknowledged but not trusted: the CLI answers `success` to
            // requests it then ignores, so the ladder is timed from when the
            // request was *sent* and the turn's own `result` is the only proof
            // an interrupt landed.
            Event::ControlResponse => {
                tracing::debug!(session = %self.id, response = %clip(line), "claude answered a control request")
            }
            Event::BackgroundTasks { tasks } => {
                let changed = self.background.snapshot(tasks);
                self.after_background(changed).await;
            }
            Event::TaskStarted { task } => {
                let changed = self.background.observe(task);
                self.after_background(changed).await;
            }
            // Straight to the mirror rather than through `after_background`: the
            // list has not moved, so there is nothing to log and nothing parked
            // on it to wake — and these arrive every few seconds per task, which
            // is a rate worth keeping off both. `publish` writes the shared
            // struct readers already poll, so the cost is one lock.
            Event::TaskProgress { progress } => {
                if self.background.progress(progress) {
                    self.publish();
                }
            }
            Event::TaskFinished { id } => {
                let changed = self.background.remove(&id);
                self.after_background(changed).await;
            }
            Event::Speech => {
                // The result claude built from an answer comes back as a `user`
                // line: the question it answers is over, by whatever path.
                if !self.asked.is_empty() {
                    let answered = answered_in(line);
                    let settled: Vec<String> = self
                        .asked
                        .iter()
                        .filter(|question| question.tool_use_id.as_ref().is_some_and(|id| answered.contains(id)))
                        .map(|question| question.request_id.clone())
                        .collect();
                    for request_id in settled {
                        self.withdraw(&request_id, QuestionState::Cancelled, "claude has a result for it");
                    }
                }
                self.on_speech()
            }
            Event::Permission(request) => {
                if request.is_question() {
                    self.ask(&request);
                } else {
                    self.refuse(&request).await;
                }
            }
            Event::ControlRequest { request_id } => self.decline_request(request_id).await,
            Event::ControlCancel { request_id } => self.withdraw(&request_id, QuestionState::Cancelled, "claude withdrew it"),
            Event::Unparseable => tracing::debug!(session = %self.id, line = %clip(line), "claude wrote a line that is not JSON"),
            // Everything else — a subagent's forwarded stream, stream_event, the
            // other system subtypes, and whatever the next CLI release invents —
            // is in the transcript the feed renders, or will be. A think the
            // session's own thread is in the middle of went to the draft above;
            // the only job left here is the `touch`, which is what the stall
            // watchdog reads.
            Event::Housekeeping | Event::Other => {}
        }
    }

    /// What claude is minding in the background has changed; say so.
    ///
    /// The only thing that *acts* on it is a settings change that has been parked
    /// waiting for the tasks to end, which is why the pump is only woken when the
    /// map empties. Everything else about a background task is claude's business:
    /// remote-hand neither starts nor ends them, it just declines to pull the
    /// process out from under one — and now describes them while they run.
    ///
    /// `changed` covers a description arriving for a task already known as well
    /// as the list itself moving, because both change what a row should say; a
    /// snapshot that repeats what was already published publishes nothing.
    async fn after_background(&mut self, changed: bool) {
        if !changed {
            return;
        }
        tracing::debug!(session = %self.id, tasks = self.background.len(), "claude's background task list changed");
        self.publish();
        if self.background.is_empty() && self.awaiting_background {
            self.pump().await;
        }
    }

    /// `system:init`: the process is listening, and it has named the session.
    ///
    /// The id in this line is authoritative. remote-hand asks for one with
    /// `--resume` or `--session-id`, but which id the CLI actually writes under
    /// is its decision — a resume it cannot satisfy in place is answered by
    /// forking into a new one — and following the wrong file means the user
    /// watches a session that has quietly stopped replying. Note that `init` is
    /// *not* the first line: hook events precede it, so it is matched on rather
    /// than waited for positionally.
    /// Learn the model name `init` reports, *beside* the one the spawn asked for.
    ///
    /// The spawn may have said `haiku` and the process answers as
    /// `claude-haiku-4-5-…`: same model, two names, and a send may carry either
    /// — the composer's own choice, or the name it read back out of the
    /// transcript. So both are kept and [`Fingerprint::satisfied_by`] compares
    /// against both.
    ///
    /// It used to *replace* the alias, which quietly made a resident stop being
    /// one: every send still saying `fable` then read as a settings change, so
    /// the process was restarted — taking its background work with it — and
    /// anything queued behind the message that caused it was failed outright,
    /// having no environment of its own to be respawned for. Eight turns and
    /// eight respawns in the flight recording that found it.
    ///
    /// Written back to the journal as well as held here, so the next server
    /// starts out knowing both names rather than re-learning the second one at
    /// the cost of a restart. One rewrite, once per process.
    ///
    /// A model name is not on its own a fingerprint, so an *unknown* live is
    /// left unknown: filling in the one field this line carries and defaulting
    /// the other five — the account above all — is precisely the guess that
    /// [`Fingerprint::accepted_by`] exists to refuse.
    fn canonicalize_model(&mut self, model: Option<String>) {
        let Some(model) = model else { return };
        let Some(live) = self.live.as_mut() else { return };
        if live.model.as_deref() == Some(model.as_str()) || live.resolved.as_deref() == Some(model.as_str()) {
            return;
        }
        tracing::debug!(session = %self.id, asked = ?live.model, answered = %model, "claude named the model it is running");
        live.resolved = Some(model);
        self.note_fingerprint();
    }

    /// Act on a `system:init`: follow the id it names, and unblock the pump.
    ///
    /// # An `init` in the middle of a session's life
    ///
    /// Not an anomaly, and not only a background task's doing: a message
    /// buffered in claude's stdin — every steered one — is answered with a
    /// fresh `system:init` of its own before its assistant lines, verified
    /// against claude 2.1.231. So this runs again, mid-life, once per steered
    /// message, and must do nothing: the id is the same, so `adopt` is not
    /// reached; the model is the same, so `canonicalize_model` is a no-op; and
    /// `ready` is already true, so the pump below — which would open a *second*
    /// turn for a message this driver has already opened one for — is skipped.
    /// That gate is load-bearing, not defensive.
    async fn on_init(&mut self, session: Option<&str>) {
        if let Some(id) = session {
            if id != self.id {
                self.adopt(id).await;
            }
        }
        // …with the one exception that makes steering safe: an `init` while a
        // *provisional* turn is open is claude saying it started a turn of its
        // own for the message it was handed, which is the proof that turn was
        // waiting for. Nothing else in the stream can say it — background
        // chatter and tool progress go on whether a turn is running or not, so
        // "claude said something" is not evidence and this deliberately is.
        if let Some(turn) = self.turn.as_mut().filter(|turn| turn.provisional) {
            turn.provisional = false;
            let at = turn.at;
            tracing::debug!(session = %self.id, "claude opened a turn of its own for the steered message");
            self.recorder
                .lifecycle(Stage::Absorb, Some("claude opened a turn of its own for the steered message"));
            // Only now is there a turn a restart could reconstruct: see
            // [`Driver::start_delivered_turn`] for why a provisional one is
            // kept out of the journal.
            self.note_progress(Some(at));
        }
        if !self.ready {
            self.ready = true;
            // A process that got this far started successfully, whatever it took
            // to get here: the recovery ladder is about resumes that *cannot*
            // start, so reaching `init` puts it back on its first rung and ends
            // the episode. The breaker's window is not reset — a session that
            // dies after init four times in five minutes is still a session in
            // a crash loop.
            self.rung = 0;
            self.recovering = false;
            self.pump().await;
        }
    }

    /// Follow claude to the session id it chose instead.
    ///
    /// Everything keyed by the id moves together, and the claim is re-taken
    /// before the old one is dropped so the session is never momentarily
    /// unowned. The account binding is deliberately *not* moved: it lives in a
    /// store this task holds no handle on, and a resume that forked has a
    /// bigger problem than its billing record — the recovery ladder that owns
    /// that case is the next phase's.
    async fn adopt(&mut self, id: &str) {
        tracing::warn!(session = %self.id, adopted = %id, "claude answered under a different session id; following it");
        // The recording keeps the name it was opened under — one file per
        // session, whatever the CLI renames it to — so this line is the only
        // thing that says the transcript being followed from here on is a
        // different file.
        self.recorder
            .lifecycle(Stage::Adopt, Some(&format!("claude answered as {id} instead of {}", self.id)));
        self.recorder.follow(id);
        let claim = match lock::acquire(&self.registry.lock_dir, Source::Claude, id) {
            Ok(lock::Outcome::Acquired(claim)) => Some(claim),
            other => {
                // Losing the claim degrades a cross-instance guarantee for a
                // process that is already running, which is not worth killing a
                // live session over.
                tracing::warn!(session = %id, busy = ?other.is_ok(), "cannot claim the session claude adopted");
                None
            }
        };
        {
            let mut residents = self.registry.residents.lock().expect("residents poisoned");
            if let Some(handle) = residents.remove(&self.id) {
                residents.insert(id.to_string(), handle);
            }
        }
        {
            let mut runs = self.registry.runs.lock().expect("injections poisoned");
            if let Some(run) = runs.remove(&self.registry.key_of(Source::Claude, &self.id)) {
                run.state.lock().expect("run state poisoned").current_id = id.to_string();
                runs.insert(self.registry.key_of(Source::Claude, id), run);
            }
        }
        self.registry.journal.repoint(&self.record, id);
        // The page following the old id would otherwise show a think in
        // flight for ever; the one following the new id sees the rest of it.
        self.draft.retarget(self.registry.draft_channel(id));
        if let Err(error) = crate::sessions::owned_sessions::record(&self.registry.paths, Source::Claude, id).await {
            tracing::error!(session = %id, %error, "cannot record the adopted claude session as ours; it will list as external");
        }
        self.id = id.to_string();
        drop(std::mem::replace(&mut self.claim, claim));
    }

    // ------------------------------------------------------------ limits

    /// Who this resident bills, as the limit log names it, and what it runs.
    ///
    /// The live fingerprint when there is one — the account and the model the
    /// process was *started* with, which are the ones a limit is about. A
    /// resident inherited through an attach and not yet described has none, so
    /// the session's binding stands in for the account and the model is
    /// unknown rather than guessed.
    ///
    /// `None` when the limit is about no login the log names: a session with no
    /// account in this instance's own root runs on the root's own login, and
    /// filing it under the local login would sign the user's `~/.claude` out
    /// for somebody else's failure (see [`rest::claude_limit_account`]).
    async fn limit_subject(&self) -> Option<(String, Option<String>)> {
        let (account, model) = match &self.live {
            Some(live) => (live.account.clone(), live.resolved.clone().or_else(|| live.model.clone())),
            None => (
                self.registry.accounts.session_account_id_for(&self.id, AccountProvider::Claude).await,
                None,
            ),
        };
        // Which root it runs in matters only when there is no account; read the
        // way `usage_login` reads it, so a reading and a limit agree on whose.
        let root = match (&account, &self.recipe) {
            (Some(_), _) => None,
            (None, Some(recipe)) => Some(recipe_root(&recipe.env)),
            (None, None) => crate::sessions::claude::find_claude_session(&self.registry.paths, &self.id)
                .await
                .map(|found| found.root),
        };
        let subject = rest::claude_limit_account(account.as_deref(), root);
        if subject.is_none() {
            tracing::info!(session = %self.id, "a limit on this instance's own root login with no account bound; not filed under the local login");
        }
        subject.map(|account_id| (account_id, model))
    }

    /// Record a `result` that says an account limit was hit, and say which.
    ///
    /// An error result is read in full; one the CLI labelled a success only
    /// when its text is the CLI's own limit sentence (see
    /// [`limits::is_definitive`]), because a successful turn's text is the
    /// model's answer and an answer may well be *about* rate limits.
    ///
    /// A weekly result borrows its reset time from a `rate_limit_event` of
    /// unknown scope seen just before it, when its own words give none — the
    /// event knew when, the result knows whose. See [`limits::with_hint`].
    ///
    /// The verdict comes back for [`Driver::rotate_after_limit`]; `None` when
    /// the result was not a limit.
    async fn note_result_limit(&mut self, is_error: bool, detail: Option<&str>, api_status: Option<u16>) -> Option<Verdict> {
        // Taken whatever this result turns out to be: a hint explains the one
        // result after it, and no later one.
        let hint = self.limit_hint.take(Instant::now());
        let text = detail.unwrap_or_default();
        if !is_error && !limits::is_definitive(text) {
            return None;
        }
        limits::classify(text, api_status)?;
        let verdict = limits::read_text(text, api_status)?;
        let verdict = limits::with_hint(verdict, hint.as_ref());
        // The verdict still goes back to the rotation; only the filing needs a
        // login to file it under.
        let Some((account_id, model)) = self.limit_subject().await else {
            return Some(verdict);
        };
        let raw = match api_status {
            Some(status) => format!("{text}\n[api_error_status {status}]"),
            None => text.to_string(),
        };
        self.registry.record_limit(Observation {
            account_id,
            session_id: Some(self.id.clone()),
            source: "claude:result",
            model,
            verdict: verdict.clone(),
            raw,
            reason: String::new(),
            may_rest: true,
        });
        Some(verdict)
    }

    /// File a `rate_limit_event`'s windows under the login this process runs on.
    ///
    /// Every event carries them, `allowed` as much as `rejected`, and they are
    /// the reading the quota panel now lives on: made by the credential actually
    /// being billed, at no cost, and with no second login to keep alive. See
    /// [`crate::quota::readings`].
    async fn note_usage(&self, info: &Value) {
        let windows = crate::quota::readings::windows_from_rate_limit_info(info);
        if windows.is_empty() {
            return;
        }
        let Some(login) = self.usage_login().await else {
            tracing::debug!(session = %self.id, "a quota reading from a session on the managed root's own login; not filed");
            return;
        };
        if let crate::quota::UsageLogin::Account(id) = &login {
            // Only for an account that still exists: a reading must not
            // recreate the directory of one deleted under a running session.
            if self.registry.accounts.get(id).await.is_none() {
                return;
            }
        }
        // A reading with room in every window may be the account a held
        // session is waiting for.
        let headroom = windows.iter().all(|window| window.used_percent < rotation::EXHAUSTED_PERCENT);
        self.registry.quota.ingest_session(login, windows);
        if headroom {
            self.registry.recheck_holds_after_reading();
        }
    }

    /// Which login this process bills, as the quota readings name it.
    ///
    /// The account the process was started with, or the session's binding when
    /// nothing was written down (see [`Driver::limit_subject`]). With no account,
    /// the root decides: the user's own `~/.claude` is the local login, while
    /// this instance's managed root runs on whatever login that root holds and
    /// is nobody's to file under. The root is read off the recipe's environment
    /// when this server started the process, and off where the transcript lives
    /// when it inherited one.
    async fn usage_login(&self) -> Option<crate::quota::UsageLogin> {
        let account = match &self.live {
            Some(live) => live.account.clone(),
            None => self.registry.accounts.session_account_id_for(&self.id, AccountProvider::Claude).await,
        };
        let root = match (&account, &self.recipe) {
            (Some(_), _) => None,
            (None, Some(recipe)) => Some(recipe_root(&recipe.env)),
            (None, None) => crate::sessions::claude::find_claude_session(&self.registry.paths, &self.id)
                .await
                .map(|found| found.root),
        };
        crate::quota::readings::session_login(account.as_deref(), root)
    }

    /// Record a refused `rate_limit_event`. Recording only.
    ///
    /// A weekly window whose name does not say which models it covers is
    /// logged under scope `unknown` and rests nothing: it is held as a hint for
    /// the `result` that follows, which does say (see [`limits::WindowHint`]).
    async fn note_rate_limit(&mut self, info: &Value) {
        let Some(reading) = limits::read_rate_limit_event(info) else { return };
        let window = info.get("rateLimitType").and_then(Value::as_str).unwrap_or("unknown");
        let status = info.get("status").and_then(Value::as_str).unwrap_or("unknown");
        let may_rest = reading.hint.is_none();
        if let Some(hint) = reading.hint {
            self.limit_hint.hold(hint, Instant::now());
        }
        let Some((account_id, model)) = self.limit_subject().await else { return };
        self.registry.record_limit(Observation {
            account_id,
            session_id: Some(self.id.clone()),
            source: "claude:rate_limit_event",
            model,
            verdict: reading.verdict,
            // The whole payload, `unifiedWindows` and all: every window's
            // utilization at the moment of refusal is worth more later than
            // the one line that names this one.
            raw: info.to_string(),
            reason: format!("rate_limit_event: {window} window {status}"),
            may_rest,
        });
    }

    /// Record a death whose stderr says the credential was refused.
    ///
    /// Only `auth`: a process that dies of anything else is the recovery
    /// ladder's business and says nothing about the account. Nothing changes
    /// about that ladder either — it still resumes with the same credential.
    async fn note_exit_limit(&self) {
        if self.retiring {
            return;
        }
        let reason = self.death_reason();
        if !reason.lines().any(|line| limits::classify(line, None) == Some(LimitClass::Auth)) {
            return;
        }
        let Some((account_id, model)) = self.limit_subject().await else { return };
        self.registry.record_limit(Observation {
            account_id,
            session_id: Some(self.id.clone()),
            source: "claude:exit",
            model,
            verdict: Verdict {
                class: LimitClass::Auth,
                scope: limits::SCOPE_ALL.to_string(),
                until_ms: None,
            },
            raw: reason,
            reason: String::new(),
            may_rest: true,
        });
    }

    /// `result`: the turn is over, whatever else the process goes on to do.
    async fn on_result(&mut self, is_error: bool, subtype: &str, detail: Option<&str>, api_status: Option<u16>) {
        // Whether or not a turn is in flight here: a `result` is claude saying
        // nothing more is coming, which is what closes the draft — no think is
        // in flight after it. See `drafts`.
        self.draft.end();
        // A question cannot outlive the turn that asked it. One still open here
        // ended without an answer — a stop, most likely, whose cancel this
        // driver did not see — and nobody may answer it into the next turn.
        self.close_questions(QuestionState::Cancelled, "the turn it was asked in ended");
        // Before the turn is taken, and whether or not there is one: a turn
        // claude opened on its own can hit a limit as well as one this server
        // asked for. A turn being stopped from here cannot — its failure is the
        // Stop, not the account.
        let limit = if !self.turn.as_ref().is_some_and(|turn| turn.ladder.is_some()) {
            self.note_result_limit(is_error, detail, api_status).await
        } else {
            None
        };
        let Some(turn) = self.turn.take() else {
            // A result with no turn is a turn that already ended — the ladder
            // killed it, a replay gap swallowed the message that opened it, or
            // a settle judged a steered message absorbed a moment before its
            // own answer arrived. Nothing to close, but it is still proof that
            // claude is between turns, so the queue is pumped: the alternative
            // is a driver holding messages for a process that is sitting idle,
            // which is the one failure mode this whole file exists to refuse.
            tracing::debug!(session = %self.id, "a result arrived with no turn in flight");
            self.pump().await;
            return;
        };
        let (outcome, error) = if turn.ladder.is_some() {
            (InjectionRunState::Failed, Some("interrupted from Droppen".to_string()))
        } else if is_error {
            // `result.result` carries only the first text block of the last
            // message, which makes a poor transcript and a perfectly good
            // explanation of a failure.
            let detail = detail.unwrap_or_default().trim();
            let message = if detail.is_empty() {
                format!("claude ended the turn with {subtype}")
            } else {
                format!("claude ended the turn with {subtype}\n{}", self.registry.redact(detail))
            };
            (InjectionRunState::Failed, Some(message))
        } else {
            (InjectionRunState::Ok, None)
        };
        let switches = turn.rotation_attempt;
        // Neither refused nor stopped from here, nor failed: the quiet point at
        // which the session may move ahead of a limit, before anything else
        // goes into the process.
        let ended_normally = limit.is_none() && turn.ladder.is_none() && !is_error;
        self.close_turn(&turn, outcome, error);
        // A limit on the session's own turn is where the queue used to be fed,
        // message after message, to the account that had just refused. Now the
        // pump waits for the decision, and the decision may be to move.
        if let Some(verdict) = limit {
            if self.rotate_after_limit(&verdict, switches).await {
                return;
            }
        }
        if ended_normally {
            self.rotate_ahead().await;
        }
        self.pump().await;
    }

    // ------------------------------------------------------------ rotation

    /// A limit ended this session's own turn: move the session, or hold its
    /// queue. Phase 2 of account rotation; see [`rotation`] for the rules.
    ///
    /// `switches` is how many times the chain the failed turn belongs to has
    /// already been moved, which is what the cap counts.
    ///
    /// `true` when the queue has been taken care of — a switch is under way, or
    /// the queue is held — and the caller must not pump. `false` hands the
    /// session back to the ordinary pump, which is right for everything that is
    /// not a new decision: a limit that does not trigger one, a session on no
    /// account or on one outside the pool, a switch already on its way, a
    /// queue already held against this very account.
    async fn rotate_after_limit(&mut self, verdict: &Verdict, switches: u8) -> bool {
        let Some(live) = self.live.clone() else { return false };
        let Some(from_id) = live.account.clone() else { return false };
        // Already holding against this account: the same limit again — a
        // steered message, or a turn claude opened by itself, refused by the
        // account the queue is held for. Not a new decision, not a new row,
        // not a second notification; only a later reset time is worth keeping.
        if let Some(hold) = self.hold.as_ref().filter(|hold| hold.account == from_id) {
            if verdict.until_ms.is_some_and(|until| hold.until_ms.is_none_or(|held| until > held)) {
                let mut hold = hold.clone();
                hold.until_ms = verdict.until_ms;
                self.set_hold(Some(hold));
            }
            return false;
        }
        // Already moving: the head of the queue is a continuation for another
        // account, parked behind background work. What failed is claude, still
        // on the old account, answering one of those tasks with the model that
        // ran out — the same limit, not a new decision.
        if self
            .queue
            .front()
            .is_some_and(|next| next.continuation.is_some() && next.fingerprint.account != live.account)
        {
            return false;
        }
        let accounts = self.registry.accounts.clone();
        let from = accounts.get(&from_id).await;
        let bound = accounts.session_account_id_for(&self.id, AccountProvider::Claude).await;
        let rebound = bound.as_deref().is_some_and(|bound| bound != from_id);
        if !rotation::triggers(verdict.class, from.as_ref(), rebound) {
            return false;
        }
        let Some(from) = from else { return false };
        // The rest phase 1 has just written knows the reset at least as well as
        // this result does, and often better: the refused `rate_limit_event`
        // before it carried the exact instant.
        let until_ms = accounts
            .limit_log()
            .rest_until(&from_id, &verdict.scope, rest::now_ms())
            .flatten()
            .or(verdict.until_ms);
        let hit = LimitHit::new(verdict.class, &verdict.scope, until_ms);
        let from = AccountRef {
            id: from.id,
            label: from.label,
        };
        let outcome = match self.decide(&live, &from.id, switches).await {
            Choice::Switch(to) => match self.switch_account(&live, &from, &to, &hit, switches).await {
                Ok(()) => return true,
                Err(error) => {
                    // Known limitation: a switch that fails part-way — the
                    // new account's recipe or the binding could not be written —
                    // is recorded as `no_candidate`, which is what it leaves the
                    // session as: held, on the account that refused.
                    tracing::warn!(
                        session = %self.id,
                        from = %from.id,
                        to = %to.id,
                        %error,
                        "cannot move this session to the next account; holding its queue instead"
                    );
                    RotationOutcome::NoCandidate
                }
            },
            Choice::Nowhere(outcome) => outcome,
        };
        self.hold_after_limit(from, &hit, switches, outcome).await;
        true
    }

    /// Where a session limited on `from` goes: [`rotation::choose`] over the
    /// pool as it stands, the session's own binding and the family of the model
    /// the process is running — the name `system:init` gave it first, the one
    /// the spawn asked for second, and unknown after that.
    async fn decide(&self, live: &Fingerprint, from: &str, switches: u8) -> Choice {
        let accounts = &self.registry.accounts;
        let bound = accounts.session_account_id_for(&self.id, AccountProvider::Claude).await;
        let pool = rotation::pool(accounts, &self.registry.quota).await;
        let family = rotation::model_family(live.resolved.as_deref().or(live.model.as_deref()));
        match rotation::choose(&pool, from, bound.as_deref(), family, switches, rest::now_ms()) {
            Plan::Switch(id) => Choice::Switch(
                pool.iter()
                    .find(|account| account.id == id)
                    .map(|account| AccountRef {
                        id: account.id.clone(),
                        label: account.label.clone(),
                    })
                    .unwrap_or_else(|| AccountRef { label: id.clone(), id }),
            ),
            Plan::NoCandidate => Choice::Nowhere(RotationOutcome::NoCandidate),
            Plan::CapReached => Choice::Nowhere(RotationOutcome::CapReached),
        }
    }

    /// Move this session from `from` to `to` and continue it there.
    ///
    /// In order: a recipe for `to` (the failed process' own, retargeted), the
    /// binding, every queued message pointed at `to`, the decision written down
    /// and announced, and a continuation at the head of the queue. The
    /// continuation needs a process `to` bills, so the fingerprint mismatch
    /// retires the old one and `--resume`s the conversation under `to` —
    /// straight away for a limit that covers every model, or once claude's
    /// background work is done for one that covers a single family.
    ///
    /// Nothing is bound until the recipe exists, so a failure before that leaves
    /// the session where it was.
    async fn switch_account(
        &mut self,
        live: &Fingerprint,
        from: &AccountRef,
        to: &AccountRef,
        hit: &LimitHit,
        switches: u8,
    ) -> Result<(), InjectError> {
        let recipe = match self.recipe.clone() {
            Some(base) => self.registry.retarget_recipe(&self.id, &base, &from.id, &to.id).await?,
            None => {
                let cwd = self.journalled_cwd();
                self.registry.recipe_from_settings(&self.id, live, cwd.as_deref(), &to.id).await?
            }
        };
        self.registry
            .accounts
            .bind_session_account(&self.id, Some(&to.id), AccountProvider::Claude)
            .await?;
        self.retarget_queue(&from.id, &recipe);
        let wait = rotation::wait_policy(&hit.scope);
        let interrupted = match wait {
            Wait::Now => self.interrupted_tasks().await,
            Wait::ForBackground => Vec::new(),
        };
        let mut note = Continuation {
            record: None,
            from: from.label.clone(),
            to: to.label.clone(),
            reason: hit.reason.clone(),
            until: hit.until.clone(),
        };
        let text = rotation::continuation_prompt(&note.from, &note.to, &note.reason, note.until.as_deref(), &interrupted);
        let decision = rotation::Decision {
            session_id: self.id.clone(),
            at_ms: rest::now_ms(),
            from: from.clone(),
            to: to.clone(),
            class: hit.class,
            scope: hit.scope.clone(),
            until_ms: hit.until_ms,
            interrupted_tasks: u32::try_from(interrupted.len()).unwrap_or(u32::MAX),
            outcome: RotationOutcome::Switched,
            trigger: RotationTrigger::Limit,
            used_percent: None,
        };
        let notice = rotation::notice(RotationOutcome::Switched, &from.label, &to.label, &hit.reason, hit.until.as_deref());
        note.record = self.registry.record_rotation(&decision, &notice);
        self.recorder.lifecycle(
            Stage::Rotate,
            Some(&format!(
                "{} limit ({}) on {}: moving to {}, {}; attempt {} of {}",
                hit.class.as_str(),
                hit.scope,
                from.id,
                to.id,
                match wait {
                    Wait::Now => format!("now, interrupting {} background task(s)", interrupted.len()),
                    Wait::ForBackground => format!("after {} background task(s)", self.background.len()),
                },
                switches.saturating_add(1),
                rotation::ROTATION_CAP,
            )),
        );
        let continuation = Pending {
            id: new_uuid(),
            text,
            fingerprint: recipe.fingerprint.clone(),
            recipe: Some(recipe.clone()),
            rotation_attempt: switches.saturating_add(1),
            continuation: Some(note),
        };
        match wait {
            Wait::Now => {
                // Steered messages went into a process that is about to go, so
                // they are undelivered again — and they were sent after the
                // message the continuation stands in for, so they go behind it.
                self.requeue_delivered();
                // Known limitation: the old process still holds these in its stdin
                // until it exits, and claude may open a turn for one (refused by the
                // same limit) before it goes. The resumed process then receives the
                // same user message a second time. Not verified against the CLI's
                // behaviour at stdin EOF.
                if self.respawn_for(continuation, &recipe).await {
                    self.pump().await;
                }
            }
            Wait::ForBackground => {
                // The ordinary settings-change park does the waiting: the head
                // of the queue no longer fits the process, and the pump holds it
                // there while claude minds background work, up to the ceiling.
                //
                // Known limitation: messages already steered into the old
                // process' stdin run there, on the account that refused, while
                // this waits — nothing can take a line back out of a pipe.
                self.queue.push_front(continuation);
                self.publish();
                self.note_progress(None);
                self.pump().await;
            }
        }
        Ok(())
    }

    /// Nowhere to move the session: say so, and hold the queue.
    async fn hold_after_limit(&mut self, from: AccountRef, hit: &LimitHit, switches: u8, outcome: RotationOutcome) {
        let notice = rotation::notice(outcome, &from.label, &from.label, &hit.reason, hit.until.as_deref());
        let decision = rotation::Decision {
            session_id: self.id.clone(),
            at_ms: rest::now_ms(),
            from: from.clone(),
            to: from.clone(),
            class: hit.class,
            scope: hit.scope.clone(),
            until_ms: hit.until_ms,
            interrupted_tasks: 0,
            outcome,
            trigger: RotationTrigger::Limit,
            used_percent: None,
        };
        self.registry.record_rotation(&decision, &notice);
        self.recorder.lifecycle(
            Stage::Rotate,
            Some(&format!(
                "{} limit ({}) on {}: {}; holding {} queued message(s)",
                hit.class.as_str(),
                hit.scope,
                from.id,
                outcome.as_str(),
                self.queue.len()
            )),
        );
        self.set_hold(Some(Hold {
            account: from.id,
            label: from.label,
            class: hit.class,
            scope: hit.scope.clone(),
            until_ms: hit.until_ms,
            switches,
            continue_failed: true,
        }));
        // Steered messages are in the old process' stdin and run whatever this
        // driver decides (the known limitation above); the pump opens their turns and then stops
        // at the hold.
        self.pump().await;
    }

    /// Ask a held queue's question again: has its limit lifted, or has an
    /// account appeared that the session can move to?
    ///
    /// Every watchdog tick, and whenever something that could change the answer
    /// happens elsewhere — an account's rest cleared, an account added or
    /// re-authorized, a quota reading with headroom
    /// ([`InjectionRegistry::recheck_holds`]). An answer of "still nowhere"
    /// decides nothing: no row, no notification — the first decision already
    /// said so.
    async fn recheck_hold(&mut self) {
        let Some(hold) = self.hold.clone() else { return };
        // From idle only. A turn in flight is claude answering something the
        // old process already had; moving now would cut it off.
        if self.turn.is_some() || self.replaying {
            return;
        }
        let Some(live) = self.live.clone() else { return };
        if live.account.as_deref() != Some(hold.account.as_str()) {
            // The process is no longer the one the queue was held against.
            self.set_hold(None);
            self.pump().await;
            return;
        }
        let now = rest::now_ms();
        let lapsed = hold.until_ms.is_some_and(|until| now >= until);
        let resting = self.registry.accounts.limit_log().rest_until(&hold.account, &hold.scope, now).is_some();
        if lapsed || !resting {
            self.lift_hold(hold).await;
            return;
        }
        let Choice::Switch(to) = self.decide(&live, &hold.account, hold.switches).await else {
            return;
        };
        let from = AccountRef {
            id: hold.account.clone(),
            label: hold.label.clone(),
        };
        let hit = LimitHit::new(hold.class, &hold.scope, hold.until_ms);
        self.set_hold(None);
        if let Err(error) = self.switch_account(&live, &from, &to, &hit, hold.switches).await {
            tracing::warn!(session = %self.id, to = %to.id, %error, "cannot move a held session to the account that freed up");
            self.set_hold(Some(hold));
        }
    }

    /// The held account's limit has lifted and nothing took the session over:
    /// continue the failed message there, then let the queue go.
    async fn lift_hold(&mut self, hold: Hold) {
        self.set_hold(None);
        if hold.continue_failed {
            if let Some(live) = self.live.clone() {
                let fingerprint = Fingerprint { resolved: None, ..live };
                let recipe = self.recipe.clone().filter(|recipe| recipe.fingerprint.account == fingerprint.account);
                self.queue.push_front(Pending {
                    id: new_uuid(),
                    text: rotation::resumed_prompt(&hold.label),
                    fingerprint,
                    recipe,
                    rotation_attempt: hold.switches,
                    continuation: None,
                });
            }
        }
        tracing::info!(session = %self.id, account = %hold.account, queued = self.queue.len(), "a held account's limit has lifted; continuing on it");
        self.recorder
            .lifecycle(Stage::Rotate, Some(&format!("{}'s limit has lifted: continuing there", hold.account)));
        self.publish();
        self.note_progress(self.turn.as_ref().map(|turn| turn.at));
        self.pump().await;
    }

    /// Let a held queue go because somebody acted.
    ///
    /// When the thing that released it is a message sent under another account
    /// — the user has moved the session by hand — the held messages follow it
    /// there rather than going back to the account that refused them. No
    /// continuation is queued: the person's own message is what comes next.
    fn release_hold(&mut self, hold: Hold, newcomer: Option<&Pending>, why: &str) {
        let elsewhere = newcomer
            .and_then(|pending| pending.recipe.as_ref())
            .filter(|recipe| recipe.fingerprint.account.is_some() && recipe.fingerprint.account.as_deref() != Some(hold.account.as_str()));
        if let Some(recipe) = elsewhere {
            let recipe = recipe.clone();
            self.retarget_queue(&hold.account, &recipe);
        }
        tracing::info!(session = %self.id, account = %hold.account, queued = self.queue.len(), why, "released a queue held after a limit");
        self.recorder.lifecycle(Stage::Rotate, Some(&format!("released the held queue: {why}")));
    }

    /// Change the hold, and the journal's copy of it.
    fn set_hold(&mut self, hold: Option<Hold>) {
        self.hold = hold;
        self.registry.journal.mark_hold(&self.record, self.hold.clone());
    }

    /// Take the hold out, if there is one, clearing the journal's copy too.
    fn take_hold(&mut self) -> Option<Hold> {
        let hold = self.hold.take();
        if hold.is_some() {
            self.registry.journal.mark_hold(&self.record, None);
        }
        hold
    }

    // ------------------------------------------------------ ahead of a limit

    /// Move this session off its account before a window of it runs out.
    ///
    /// Asked only at a quiet point — right after a turn ended normally, and
    /// before the pump starts the next one — and never while claude minds
    /// background work: the point of moving early is that nothing is cut off,
    /// so a subagent still running puts the question off to the next quiet
    /// point. The rules themselves are [`rotation::plan_ahead`]'s.
    ///
    /// Only the binding and the queue move here. The process is replaced the
    /// next time the pump has something for it: the head of the queue then
    /// names the new account, and the ordinary settings-change path
    /// `--resume`s the conversation under it. No continuation prompt — nothing
    /// failed, so there is nothing to carry on from. `true` when it moved.
    async fn rotate_ahead(&mut self) -> bool {
        let Some(threshold) = self.registry.rotate_ahead else { return false };
        if self.turn.is_some()
            || self.replaying
            || self.hold.is_some()
            || self.awaiting_background
            || !self.delivered_ahead.is_empty()
            || !self.asked.is_empty()
        {
            return false;
        }
        let Some(live) = self.live.clone() else { return false };
        let Some(from_id) = live.account.clone() else { return false };
        // Already on its way somewhere: a waiting message names another
        // account — a rotation's continuation, or somebody's own choice.
        if self.queue.iter().any(|pending| pending.fingerprint.account != live.account) {
            return false;
        }
        let family = rotation::model_family(live.resolved.as_deref().or(live.model.as_deref()));
        let now = rest::now_ms();
        // The cheap question first, at every quiet point: this account's own
        // reading. The binding, the log, the background work on disk and the
        // whole pool only when it is near.
        let own: Vec<rotation::Window> = self
            .registry
            .quota
            .cached(&crate::quota::UsageLogin::Account(from_id.clone()))
            .windows
            .iter()
            .map(rotation::Window::from_quota)
            .collect();
        if rotation::near_window(&own, family, threshold, now).is_none() {
            return false;
        }
        let accounts = self.registry.accounts.clone();
        // A binding that names another account already is where the next
        // message goes; there is nothing to decide.
        let bound = accounts.session_account_id_for(&self.id, AccountProvider::Claude).await;
        if bound.as_deref() != Some(from_id.as_str()) {
            return false;
        }
        let log = accounts.rotation_log();
        let guards = AheadGuards {
            background: self.minding_background().await,
            last_ahead_ms: log.last_ahead_ms(&self.id),
            bound_by_hand_ms: log.bound_by_hand_ms(&self.id),
        };
        let pool = rotation::pool(&accounts, &self.registry.quota).await;
        match rotation::plan_ahead(&pool, &from_id, family, threshold, guards, now) {
            AheadPlan::Stay => false,
            AheadPlan::Defer(near, why) => {
                tracing::debug!(
                    session = %self.id,
                    account = %from_id,
                    window = %near.label,
                    used = near.used_percent,
                    ?why,
                    "a window of this session's account is about to run out; not moving it yet"
                );
                false
            }
            AheadPlan::NoCandidate(near) => {
                // Once per crossing: the same window of the same account,
                // until it resets, says the same thing at every quiet point.
                let crossing = format!("{from_id} {} {}", near.label, near.resets_at_ms.unwrap_or_default());
                if self.ahead_noted.as_deref() != Some(crossing.as_str()) {
                    tracing::info!(
                        session = %self.id,
                        account = %from_id,
                        window = %near.label,
                        used = near.used_percent,
                        threshold,
                        "a window of this session's account is about to run out and no pool account is known to have room; staying"
                    );
                    self.ahead_noted = Some(crossing);
                }
                false
            }
            AheadPlan::Switch(to, near) => {
                let named = |id: &str| AccountRef {
                    id: id.to_string(),
                    label: pool
                        .iter()
                        .find(|account| account.id == id)
                        .map(|account| account.label.clone())
                        .unwrap_or_else(|| id.to_string()),
                };
                let (from, to) = (named(&from_id), named(&to));
                match self.switch_ahead(&live, &from, &to, &near).await {
                    Ok(()) => true,
                    Err(error) => {
                        tracing::warn!(
                            session = %self.id,
                            from = %from.id,
                            to = %to.id,
                            %error,
                            "cannot move this session ahead of its account's limit; staying"
                        );
                        false
                    }
                }
            }
        }
    }

    /// Move this session from `from` to `to` ahead of `near` running out: a
    /// recipe for `to`, the binding, every queued message pointed at `to`, and
    /// the decision written down and announced.
    ///
    /// The recipe is built first, as [`Driver::switch_account`] builds it, so a
    /// failure leaves the session bound where it was.
    async fn switch_ahead(&mut self, live: &Fingerprint, from: &AccountRef, to: &AccountRef, near: &rotation::Near) -> Result<(), InjectError> {
        let recipe = match self.recipe.clone() {
            Some(base) => self.registry.retarget_recipe(&self.id, &base, &from.id, &to.id).await?,
            None => {
                let cwd = self.journalled_cwd();
                self.registry.recipe_from_settings(&self.id, live, cwd.as_deref(), &to.id).await?
            }
        };
        self.registry
            .accounts
            .bind_session_account(&self.id, Some(&to.id), AccountProvider::Claude)
            .await?;
        self.retarget_queue(&from.id, &recipe);
        let until = rotation::until_text_local(near.resets_at_ms);
        let decision = rotation::Decision {
            session_id: self.id.clone(),
            at_ms: rest::now_ms(),
            from: from.clone(),
            to: to.clone(),
            class: near.class,
            scope: near.scope.clone(),
            until_ms: near.resets_at_ms,
            interrupted_tasks: 0,
            outcome: RotationOutcome::Switched,
            trigger: RotationTrigger::Ahead,
            used_percent: Some(near.used_percent),
        };
        let notice = rotation::ahead_notice(&from.label, &to.label, near, until.as_deref());
        self.registry.record_rotation(&decision, &notice);
        self.ahead_noted = None;
        self.recorder.lifecycle(
            Stage::Rotate,
            Some(&format!(
                "{} window at {}% on {}: moving to {} ahead of the limit; {} queued message(s) follow, and the next one resumes there",
                near.label,
                rotation::percent_text(near.used_percent),
                from.id,
                to.id,
                self.queue.len()
            )),
        );
        self.publish();
        Ok(())
    }

    /// Where the journal says this process was started, for a recipe built
    /// from nothing.
    fn journalled_cwd(&self) -> Option<String> {
        self.registry
            .journal
            .read()
            .into_iter()
            .find(|record| record.run_id == self.record)
            .and_then(|record| record.cwd)
    }

    /// A recipe for a message read back out of the journal that names another
    /// account than the running process — a rotation's continuation and the
    /// messages it retargeted, parked when the server restarted — when that
    /// account is the one the session is bound to now.
    ///
    /// Built on the running process' recipe with the account swapped when this
    /// driver has one, and from the message's own settings when it does not
    /// (every re-attached driver, until a message brings one). Any other
    /// mismatch is not rebuilt, and the pump fails the message as before.
    async fn recipe_for_binding(&self, next: &Pending) -> Option<Recipe> {
        let live = self.live.as_ref()?;
        let account = next.fingerprint.account.as_deref()?;
        if !next.fingerprint.fits_but_for_login(live) {
            return None;
        }
        let bound = self.registry.accounts.session_account_id_for(&self.id, AccountProvider::Claude).await?;
        if bound != account {
            return None;
        }
        let built = match (&self.recipe, live.account.as_deref()) {
            (Some(base), Some(from)) => self.registry.retarget_recipe(&self.id, base, from, account).await,
            _ => {
                let cwd = self.journalled_cwd();
                self.registry
                    .recipe_from_settings(&self.id, &next.fingerprint, cwd.as_deref(), account)
                    .await
            }
        };
        match built {
            Ok(recipe) => {
                tracing::info!(session = %self.id, account, "rebuilt a journalled message's recipe for the account the session is bound to");
                Some(recipe)
            }
            Err(error) => {
                tracing::warn!(session = %self.id, account, %error, "cannot rebuild a journalled message for the session's account");
                None
            }
        }
    }

    /// Point every waiting message that was going to `from` at the account
    /// `template` bills instead.
    ///
    /// The environment is copied whole: it is the session's, not the
    /// message's — the Claude root, the account, the `rh-notify` token minted
    /// for this session — so every message for this session under one account
    /// has the same one. A message out of the journal has no recipe to rewrite,
    /// and only its fingerprint moves: it was accepted by the process that just
    /// failed, and the replacement is that process under another account.
    fn retarget_queue(&mut self, from: &str, template: &Recipe) {
        let mut moved = 0usize;
        for pending in self.delivered_ahead.iter_mut().chain(self.queue.iter_mut()) {
            if pending.fingerprint.account.as_deref() != Some(from) {
                continue;
            }
            pending.fingerprint.account = template.fingerprint.account.clone();
            pending.fingerprint.credential = template.fingerprint.credential.clone();
            if let Some(recipe) = pending.recipe.as_mut() {
                recipe.env = template.env.clone();
                recipe.fingerprint.account = template.fingerprint.account.clone();
                recipe.fingerprint.credential = template.fingerprint.credential.clone();
            }
            moved += 1;
        }
        if moved > 0 {
            tracing::debug!(session = %self.id, from, moved, "queued messages follow the session to its new account");
            self.note_progress(self.turn.as_ref().map(|turn| turn.at));
        }
    }

    /// The background tasks a respawn now would cut short, as the
    /// continuation lists them: what each was asked to do, and its output file
    /// when claude has written one.
    async fn interrupted_tasks(&self) -> Vec<rotation::Interrupted> {
        if self.background.is_empty() {
            return Vec::new();
        }
        let dir = self.task_output_dir().await;
        let mut tasks = Vec::with_capacity(self.background.len());
        for task in &self.background.entries {
            let output = match &dir {
                Some(dir) => {
                    let path = dir.join(format!("{}.output", task.id));
                    tokio::fs::metadata(&path).await.is_ok().then(|| path.display().to_string())
                }
                None => None,
            };
            tasks.push(rotation::Interrupted {
                description: task
                    .description
                    .0
                    .clone()
                    .filter(|description| !description.trim().is_empty())
                    .unwrap_or_else(|| format!("后台任务 {}", task.id)),
                output,
            });
        }
        tasks
    }

    /// A continuation about to restart the process, after waiting for
    /// background work: if the wait ran out with tasks still going, say which
    /// in its text and in the log.
    async fn account_for_interruptions(&mut self, mut next: Pending) -> Pending {
        if self.background.is_empty() {
            return next;
        }
        let Some(note) = next.continuation.clone() else { return next };
        let interrupted = self.interrupted_tasks().await;
        next.text = rotation::continuation_prompt(&note.from, &note.to, &note.reason, note.until.as_deref(), &interrupted);
        if let Some(record) = note.record {
            let count = u32::try_from(interrupted.len()).unwrap_or(u32::MAX);
            if let Err(error) = self.registry.accounts.rotation_log().set_interrupted(record, count) {
                tracing::warn!(session = %self.id, %error, "cannot record what an account switch interrupted");
            }
            self.registry.announce(Source::Claude, &self.id);
        }
        next
    }

    /// The main thread wrote a line with no turn in flight: claude opened one.
    ///
    /// # The turn nobody asked for
    ///
    /// A turn normally starts because this server wrote a prompt into stdin,
    /// which is why [`Driver::start_turn`] was for a long time the only place
    /// one could begin. claude 2.1.231 has a second way in, and it is not rare:
    /// a background task that finishes while the session is between turns wakes
    /// the main agent with its `task_notification`, and the CLI then runs a
    /// whole turn on its own — `system:init`, assistant messages, tool results,
    /// `result` — with nothing on stdin to have started it.
    ///
    /// Read off a real session (`3c002577`, 2026-08-30): three `result` lines
    /// went out where this server had opened two turns, and the turn it had not
    /// opened ran for two and a half minutes. Two user-visible failures came out
    /// of that gap, and both are this function:
    ///
    /// * the session read **idle** while its main thread was working, because
    ///   the phase only ever moved when a prompt was written;
    /// * a message sent during such a turn was judged to be arriving *between*
    ///   turns, so [`Driver::pump`] wrote it straight into stdin — where the CLI
    ///   folded it into the turn already running, answered it inside somebody
    ///   else's reply, and left no `user` entry in the transcript at all. "My
    ///   message disappeared", exactly.
    ///
    /// The rule fixes both at once, and the second one without a line of new
    /// queue code: an implicit turn is a turn, so `pump` holds the next message
    /// and the machinery that already exists journals it and delivers it when
    /// the `result` lands.
    ///
    /// # What may not open one
    ///
    /// Everything hangs off the discriminator in [`classify`], and the
    /// exclusions are the load-bearing part:
    ///
    /// * a **forwarded child event** — a background subagent's stream,
    ///   republished on the parent's stdout — is that subagent working, not this
    ///   thread. It is the ordinary state of an idle session that dispatched
    ///   background work, and a turn opened on one would never end;
    /// * a **`system` line**, `task_progress` and `task_updated` included, for
    ///   the same reason;
    /// * **`init`**, pointedly. Every process writes one at start-up, so a turn
    ///   opened on an init would be opened by a session that has done nothing —
    ///   and the notification-driven turn's own init arrives *before* its first
    ///   assistant line anyway, so nothing is lost by waiting for the latter;
    /// * a **`result`**, stale or duplicated: results only ever close, and a
    ///   result with no turn is handled where it always was, in
    ///   [`Driver::on_result`].
    ///
    /// A replay never reaches here at all — [`Driver::replay`] reconstructs
    /// rather than opens — so a re-attach mid-implicit-turn is decided by the
    /// same evidence and the same `replay_open` policy as any other turn.
    fn on_speech(&mut self) {
        if self.turn.is_some() {
            return;
        }
        // The idle shutdown has already closed this process' stdin and is
        // waiting for it to go. Whatever it writes on the way out belongs to the
        // ending rather than to a turn, and opening one here would have
        // [`Driver::finish`] report a shutdown this server asked for as a failed
        // injection, with a notification to match.
        if self.retiring {
            return;
        }
        tracing::info!(session = %self.id, "claude opened a turn of its own; following it");
        // Marked in the recording, because the forensic question a flight log is
        // read for — why does this session say it is working — has a different
        // answer here than two lines above: there is no `stdin` line before this
        // `turn-open`, and there never will be.
        self.recorder
            .lifecycle(Stage::TurnOpen, Some("claude opened a turn of its own; nothing was written for it"));
        let turn = Turn {
            // The helper the replay's reconstruction uses, and for the same
            // reason: a run this session has already published is the one the
            // composer is looking at, and opening a second would leave the first
            // running for ever.
            run: self.launching_turn(),
            at: self.link.at(),
            seq: self.seq,
            ladder: None,
            // Real by observation. `provisional` is for a turn opened on a
            // *guess* — a steered message the CLI may have folded into somebody
            // else's turn — and the settle that judges it would close this one
            // in the middle of a long tool call. What is in flight here is not
            // in doubt: claude is talking.
            provisional: false,
            rotation_attempt: 0,
        };
        // Deliberately no journal mark. `start_turn` writes one because the
        // prompt it wrote is the only evidence its turn exists; this turn's
        // evidence is claude's own stdout, which a re-attach replays from byte
        // zero and reads as a turn in flight without any help — see
        // [`Driver::replay`]. The one case where a mark earns its keep here is a
        // message queued behind this turn, and `Cmd::Deliver` records the
        // offset when that happens.
        self.set_phase(Phase::Working);
        self.turn = Some(turn);
    }

    // ----------------------------------------------------------- questions

    /// claude stopped to ask the user something: hold the question open.
    ///
    /// The turn stays in flight — claude is blocked mid-turn, and will be until
    /// the answer is written back — so a question is *working*, not idle, to
    /// everything that reads the phase. What changes is everything that would
    /// otherwise end the process or the turn while the user thinks: the stall
    /// watchdog, the ghost-turn check, the idle retirement, a settings respawn,
    /// a move ahead of a limit. Each of them stands down while [`Driver::asked`]
    /// is not empty. A stop does not: the user may take back their own turn.
    fn ask(&mut self, request: &PermissionRequest) {
        // The `assistant` line carrying the call came first and normally
        // opened the turn already; this covers a question whose call this
        // driver never saw.
        self.on_speech();
        tracing::info!(session = %self.id, request = %request.request_id, "claude is asking the user a question");
        self.hold_question(OpenQuestion::of(request));
    }

    /// Hold a question claude is blocked on open: on record, on the mirror, on
    /// every screen following the session, and — the first time any server
    /// hears of it — on the phone. A question this driver inherited across a
    /// restart is already on record, which is what keeps the phone quiet.
    fn hold_question(&mut self, question: OpenQuestion) {
        let fresh = self.registry.questions.open(
            &question.request_id,
            &self.id,
            question.tool_use_id.as_deref(),
            &question.questions,
            questions::now_ms(),
        );
        let headline = questions::headline(&question.questions);
        self.recorder
            .lifecycle(Stage::Question, Some(&format!("{} asked: {headline}", question.request_id)));
        if fresh {
            crate::push::notify_question(self.registry.paths.clone(), Source::Claude, self.id.clone(), headline);
        }
        if !self.asked.iter().any(|open| open.request_id == question.request_id) {
            self.asked.push(question);
        }
        self.publish();
        self.announce();
    }

    /// Stop holding a question open, and say what became of it. A no-op for
    /// one this driver is not holding.
    fn withdraw(&mut self, request_id: &str, to: QuestionState, why: &str) {
        let Some(at) = self.asked.iter().position(|question| question.request_id == request_id) else {
            return;
        };
        self.asked.remove(at);
        // Only out of `pending`: an answer this driver wrote has already moved
        // the record, and claude echoing it back must not rewrite it.
        if self.registry.questions.close(request_id, to, questions::now_ms()) {
            tracing::info!(session = %self.id, request = %request_id, state = to.as_str(), why, "a question closed without an answer");
            self.recorder
                .lifecycle(Stage::Question, Some(&format!("{request_id} {}: {why}", to.as_str())));
        }
        self.publish();
        self.announce();
    }

    /// Close every question this driver is holding open.
    fn close_questions(&mut self, to: QuestionState, why: &str) {
        let open: Vec<String> = self.asked.iter().map(|question| question.request_id.clone()).collect();
        for request_id in open {
            self.withdraw(&request_id, to, why);
        }
    }

    /// Refuse a permission prompt, at once and in words the agent can act on.
    ///
    /// Every prompt but a question lands here. Before `--permission-prompt-tool`
    /// the CLI refused these itself under `--permission-mode auto`; it now asks
    /// instead, and the answer is the same refusal — remote-hand grants nothing
    /// interactively, and an approval nobody is there to give would block the
    /// turn for ever. See [`questions::permission_denial`].
    async fn refuse(&mut self, request: &PermissionRequest) {
        let message = questions::permission_denial(request);
        let line = questions::control_response(&request.request_id, &questions::deny(&message));
        tracing::info!(
            session = %self.id,
            tool = %request.tool_name,
            reason = ?request.reason,
            "claude asked for a permission Droppen does not grant; refused"
        );
        self.recorder.lifecycle(
            Stage::Permission,
            Some(&format!(
                "refused {}: {}",
                request.tool_name,
                request.reason.as_deref().unwrap_or("no reason given")
            )),
        );
        self.recorder.stdin(&line);
        if let Err(error) = self.link.send_line(&line).await {
            tracing::warn!(session = %self.id, %error, "cannot refuse a permission prompt; the process is going");
        }
    }

    /// Answer a control request this server has no handler for, so claude is
    /// not left waiting on one that will never come.
    async fn decline_request(&mut self, request_id: Option<String>) {
        let Some(request_id) = request_id else { return };
        tracing::info!(session = %self.id, request = %request_id, "claude made a control request this server does not handle; declining it");
        let line = questions::control_error(&request_id, "Droppen does not handle this request");
        self.recorder.stdin(&line);
        if let Err(error) = self.link.send_line(&line).await {
            tracing::warn!(session = %self.id, %error, "cannot decline a control request");
        }
    }

    /// Write the user's answer — or their refusal to give one — to a question
    /// claude is blocked on.
    ///
    /// The record moves first, by compare-and-set out of `pending`: of two
    /// devices answering at once, the one that loses is told who won, and
    /// nothing of its answer reaches claude. Only then is the response
    /// written; a write that fails puts the record back, and the death the
    /// failure means expires it properly a moment later.
    async fn answer(&mut self, request_id: &str, reply: Reply, by: Option<String>) -> AnswerOutcome {
        if self.replaying {
            return AnswerOutcome::NotReady;
        }
        let Some(at) = self.asked.iter().position(|question| question.request_id == request_id) else {
            return AnswerOutcome::Unknown;
        };
        let question = self.asked[at].clone();
        let (to, answer, response) = match reply {
            Reply::Answer(answer) => {
                if let Err(why) = questions::check_answer(&question.questions, &answer) {
                    return AnswerOutcome::Invalid(why);
                }
                let response = questions::allow(&question.questions, &answer);
                (QuestionState::Answered, Some(answer), response)
            }
            Reply::Dismiss => (QuestionState::Dismissed, None, questions::deny(questions::DISMISSED)),
        };
        let store = self.registry.questions.clone();
        let now = questions::now_ms();
        let stored = match store.resolve(
            request_id,
            &[QuestionState::Pending],
            to,
            answer.as_ref(),
            Some(&response),
            by.as_deref(),
            now,
        ) {
            Ok(stored) => stored,
            Err(Some(current)) => return AnswerOutcome::Resolved(current.wire()),
            // Not on record at all, which only a database that refused the
            // write when the question arrived can explain. The question is
            // real — claude is blocked on it — so it is put on record now and
            // answered the same way.
            Err(None) => {
                store.open(request_id, &self.id, question.tool_use_id.as_deref(), &question.questions, now);
                match store.resolve(
                    request_id,
                    &[QuestionState::Pending],
                    to,
                    answer.as_ref(),
                    Some(&response),
                    by.as_deref(),
                    now,
                ) {
                    Ok(stored) => stored,
                    Err(current) => return current.map_or(AnswerOutcome::Unknown, |current| AnswerOutcome::Resolved(current.wire())),
                }
            }
        };
        let line = questions::control_response(request_id, &response);
        self.recorder.lifecycle(
            Stage::Question,
            Some(&format!("{request_id} {} by {}", to.as_str(), by.as_deref().unwrap_or("someone"))),
        );
        self.recorder.stdin(&line);
        if let Err(error) = self.link.send_line(&line).await {
            tracing::warn!(session = %self.id, request = %request_id, %error, "cannot write an answer to claude");
            store.rewind(request_id, to, QuestionState::Pending, now);
            return AnswerOutcome::Unreachable;
        }
        tracing::info!(session = %self.id, request = %request_id, state = to.as_str(), "a question was answered");
        self.asked.remove(at);
        // The silence while the user thought was not claude being stuck.
        self.touch();
        self.publish();
        self.announce();
        AnswerOutcome::Delivered(stored.wire())
    }

    /// Tell the session's screens and the list that something no file shows
    /// has changed: a question opened or closed.
    fn announce(&self) {
        self.registry.announce(Source::Claude, &self.id);
    }

    // ------------------------------------------------------------ commands

    async fn on_command(&mut self, command: Cmd) {
        match command {
            Cmd::Deliver { pending, recorded } => {
                if let Some(hold) = self.take_hold() {
                    self.release_hold(hold, Some(&pending), "a new message was sent");
                }
                self.queue.push_back(*pending);
                self.publish();
                // Recorded before it is pumped, so a message that ends up
                // waiting behind a turn is on disk from the moment it arrives
                // rather than from whenever the turn in front of it ends. The
                // pump rewrites it a moment later if it goes straight in.
                self.note_progress(self.turn.as_ref().map(|turn| turn.at));
                // And only now may the send be answered: the commit above has
                // returned, so a server killed after the `200` leaves this
                // message where the next one reads. Before the pump, because a
                // pump that has to respawn for it can take seconds, and the
                // message is already safe.
                let _ = recorded.send(());
                self.pump().await;
            }
            Cmd::Drop { qid, reply } => {
                let outcome = self.drop_queued(&qid).await;
                let _ = reply.send(outcome);
            }
            Cmd::Steer { qid, reply } => {
                let outcome = self.steer(&qid).await;
                let _ = reply.send(outcome);
            }
            Cmd::Interrupt => self.interrupt().await,
            Cmd::StopTask { task_id } => self.stop_task(&task_id).await,
            Cmd::Escalate { turn, rung } => self.escalate(turn, rung),
            Cmd::Settle { turn } => self.settle_delivered(turn).await,
            Cmd::Absorb { nudged } => self.on_absorb(nudged).await,
            Cmd::Recheck => self.recheck_hold().await,
            Cmd::Answer {
                request_id,
                reply,
                by,
                respond,
            } => {
                let outcome = self.answer(&request_id, reply, by).await;
                let _ = respond.send(outcome);
            }
        }
    }

    /// Take a message out of the queue before it is written anywhere.
    ///
    /// Only ever the queue: a message that has been steered is in claude's
    /// stdin and there is no taking it back, and one whose turn has started is
    /// not this driver's to cancel — that is what Stop is for.
    async fn drop_queued(&mut self, qid: &str) -> QueueOutcome {
        if self.delivered_ahead.iter().any(|pending| pending.id == qid) {
            return QueueOutcome::AlreadySteered;
        }
        let Some(at) = self.queue.iter().position(|pending| pending.id == qid) else {
            return QueueOutcome::Unknown;
        };
        let dropped = self.queue.remove(at).expect("the index came from this queue");
        tracing::info!(session = %self.id, qid, text = %clip(&dropped.text), "a queued message was withdrawn before it went in");
        self.receipt(&dropped, ReceiptStatus::Failed);
        self.publish();
        self.note_progress(self.turn.as_ref().map(|turn| turn.at));
        // The head of the queue is what a settings-change park waits for, so
        // removing it has to let the driver ask the question again: otherwise a
        // session sits `awaitingBackground` for a message that is gone.
        self.pump().await;
        QueueOutcome::Done
    }

    /// Write a queued message to claude now, ahead of the turn in flight.
    ///
    /// The point of the whole feature: a user watching an agent go the wrong way
    /// wants the correction *in this turn's context*, not in twenty minutes when
    /// the turn ends. claude buffers a `user` line written mid-turn and runs it
    /// as the next turn, so this is a delivery, not an interruption — nothing in
    /// flight is disturbed, and the message runs before anything still queued.
    ///
    /// A message that wants different settings is not written anywhere: it
    /// needs a *different process*, and there is no stdin to write it to that
    /// would mean what the caller asked for. From idle the steer becomes that
    /// restart instead of a refusal — see [`Driver::restart_for_steer`].
    async fn steer(&mut self, qid: &str) -> QueueOutcome {
        if self.delivered_ahead.iter().any(|pending| pending.id == qid) {
            return QueueOutcome::AlreadySteered;
        }
        let Some(at) = self.queue.iter().position(|pending| pending.id == qid) else {
            return QueueOutcome::Unknown;
        };
        // "Now" overrides a queue held after a limit, as it overrides a park.
        if let Some(hold) = self.take_hold() {
            self.release_hold(hold, None, "a held message was steered");
        }
        if !self.queue[at].fingerprint.accepted_by(self.live.as_ref()) {
            return self.restart_for_steer(at).await;
        }
        // Before the write, so the fold this message may become cannot land in
        // the transcript ahead of where it is read from: see `absorb`. Taken
        // afresh when nothing else is outstanding, so a fold from an earlier
        // life of the same words — a message requeued off a dead process and
        // steered again — is behind the anchor; kept when something is, since
        // the older anchor is what still covers that.
        if self.delivered_ahead.is_empty() || !self.absorb_anchored() {
            self.anchor_absorb().await;
        }
        let pending = self.queue.remove(at).expect("the index came from this queue");
        let message = user_line(&pending.text);
        self.recorder.lifecycle(
            Stage::Steer,
            Some(&format!("{qid} was handed to claude ahead of the queue: {}", clip(&pending.text))),
        );
        self.recorder.stdin(&message);
        if let Err(error) = self.link.send_line(&message).await {
            // Nothing was written, so nothing was delivered: the message goes
            // back exactly where it was and the caller is told to try again.
            // The recv loop is about to see the same broken link and recover,
            // which is what will eventually deliver it.
            tracing::warn!(session = %self.id, qid, %error, "cannot steer a message into claude");
            self.queue.insert(at, pending);
            return QueueOutcome::Unreachable;
        }
        tracing::info!(session = %self.id, qid, "a queued message was steered into the turn in flight");
        // In claude's stdin is handed over, whatever turn it ends up running in.
        self.receipt(&pending, ReceiptStatus::Sent);
        // The same reason [`Driver::pump`] takes one: a message that fits the
        // running process describes a process equivalent to it, which is what a
        // re-attached driver needs before it can respawn anything.
        //
        // The *fingerprint* is pointedly not taken here, and the asymmetry is
        // the whole point of a steer: this message jumps the queue. Everything
        // the pump's take-once could re-judge is behind the message that healed
        // an unknown live and brings a recipe of its own; a steer is the one
        // thing that puts a message in front of messages inherited under an
        // unknown fingerprint, and narrowing live from it would fail them for
        // settings nobody ever compared them against. The recipe is only ever
        // added information, so it is safe on any order.
        if self.recipe.is_none() {
            self.recipe = pending.recipe.clone();
        }
        self.delivered_ahead.push_back(pending);
        self.arm_absorb();
        self.publish();
        self.note_progress(self.turn.as_ref().map(|turn| turn.at));
        // A steer into an *idle* resident — the queue was parked behind a
        // settings change, or the process is still starting — has nothing to
        // wait for, so its turn opens here rather than at the next `result`.
        self.pump().await;
        QueueOutcome::Done
    }

    /// Steer a message the running process cannot answer: restart for it now.
    ///
    /// The button this is behind means *get on with it*, and the case it exists
    /// for is a queue parked behind background work. A settings change waits at
    /// the head of the queue while claude minds a task — see [`Driver::pump`] —
    /// and that wait is bounded by `await_background`, which is thirty minutes.
    /// A long-lived background shell holds the whole of it.
    ///
    /// Every settings change used to be refused here, which left that user with
    /// no way out at all: the driver is `Idle` while it is parked, so Stop
    /// refuses it; the message cannot be delivered, because it does not fit the
    /// process; and the only remaining moves were to wait out the ceiling or
    /// throw the message away. Pressing the one button that says "now" is the
    /// clearest possible statement that the background work is worth less than
    /// the message, so from idle it is taken at its word and the restart
    /// happens — the confirmation the UI asks for first is where that choice is
    /// actually made.
    ///
    /// Two cases still cannot be served, and both keep the old answer:
    ///
    /// * a **turn in flight**, because a process is only ever replaced from
    ///   idle: doing it here would kill the turn the user is watching, which is
    ///   not what they asked for. It becomes possible the moment the turn ends,
    ///   by itself.
    /// * a message with **no recipe**, read back from a journal that cannot
    ///   hold the environment it needs. There is nothing to spawn from, and the
    ///   pump fails it out loud when it reaches the head.
    ///
    /// Both carry what differs, because a refusal that does not name the
    /// setting in the way is a mystery rather than a message.
    async fn restart_for_steer(&mut self, at: usize) -> QueueOutcome {
        // `live` is `Some` by construction: an unknown fingerprint accepts every
        // message, so nothing reaches here without two of them to compare.
        let differences = self
            .live
            .as_ref()
            .map(|live| self.queue[at].fingerprint.differences(live))
            .unwrap_or_else(|| "different settings".to_string());
        if self.turn.is_some() {
            return QueueOutcome::NeedsRestart { differences };
        }
        let Some(wanted) = self.queue[at].recipe.clone() else {
            return QueueOutcome::NeedsRestart { differences };
        };
        let pending = self.queue.remove(at).expect("the index came from this queue");
        tracing::info!(
            session = %self.id,
            qid = %pending.id,
            tasks = self.background.len(),
            %differences,
            "a queued message that needs a different process was steered; restarting for it now"
        );
        self.recorder.lifecycle(
            Stage::Steer,
            Some(&format!(
                "{} was steered and needs {differences}: restarting this session for it now, ahead of {} background task(s)",
                pending.id,
                self.background.len()
            )),
        );
        if !self.respawn_for(pending, &wanted).await {
            // Nothing started. `fatal` is set, the link is closed, and the
            // driver's own loop is winding the session up — the message is at
            // the head of a queue that goes with it, which is exactly what
            // `Unreachable` tells the caller.
            return QueueOutcome::Unreachable;
        }
        self.pump().await;
        QueueOutcome::Done
    }

    /// Ask claude to end the turn, and arm the ladder in case it will not.
    ///
    /// # What Stop does not stop
    ///
    /// Only the turn in flight. A message that was *steered* is already in
    /// claude's stdin, and nothing in this server can reach into another
    /// process' buffer to take it out: the CLI will run it as the next turn,
    /// interrupt or no interrupt. That is the price of steering being a
    /// delivery rather than an interruption, and it is why the row keeps
    /// showing those messages as outstanding — a user who stops a turn and sees
    /// another one start is watching something they asked for.
    ///
    /// Pressing Stop again ends that turn too, which is the way out.
    ///
    /// A Stop pressed during the launch window skips the polite rung entirely.
    /// The request is a line of stdin, and nothing reads a resident's stdin
    /// before `system:init` — so sending it would be asking a process that is
    /// not listening, and then waiting the full rung for an answer that cannot
    /// come. The ladder is armed instead, which gives the launch one step's
    /// grace to finish on its own before the first signal.
    async fn interrupt(&mut self) {
        if self.turn.is_none() {
            if self.ready {
                return; // idle between turns: there is nothing to stop
            }
            tracing::info!(session = %self.id, "stop pressed before claude said it was listening; arming the signal ladder");
            self.recorder
                .lifecycle(Stage::Interrupt, Some("stop pressed before claude said it was listening"));
            let run = self.launching_turn();
            let seq = self.seq;
            run.state.lock().expect("run state poisoned").interrupted = true;
            // No prompt has been written, so there is no offset to mark: this
            // turn exists to be stopped, not to be reconstructed after a restart.
            self.turn = Some(Turn {
                run,
                at: self.link.at(),
                seq,
                ladder: Some(0),
                provisional: false,
                rotation_attempt: 0,
            });
            self.set_phase(Phase::Working);
            self.arm(seq, 1);
            return;
        }
        // A stop takes back the turn, and the question the turn is blocked on
        // with it: claude withdraws it once the interrupt lands, and an answer
        // from a second screen in the meantime must not go in after the stop.
        self.close_questions(QuestionState::Cancelled, "the turn was stopped");
        let Some(turn) = &mut self.turn else { return };
        if turn.ladder.is_some() {
            return; // already asked; the rungs are already running
        }
        turn.ladder = Some(0);
        turn.run.state.lock().expect("run state poisoned").interrupted = true;
        let seq = turn.seq;
        let request = serde_json::json!({
            "type": "control_request",
            "request_id": format!("int-{}", new_uuid()),
            "request": {"subtype": "interrupt"},
        })
        .to_string();
        self.recorder.lifecycle(Stage::Interrupt, Some("asking claude to end the turn"));
        self.recorder.stdin(&request);
        if let Err(error) = self.link.send_line(&request).await {
            tracing::warn!(session = %self.id, %error, "cannot ask claude to stop; the ladder will have to do it");
        }
        self.arm(seq, 1);
    }

    /// Ask claude to stop one background task, and nothing else.
    ///
    /// The CLI's own `stop_task` control request, which is a different verb from
    /// [`Driver::interrupt`] in every way that matters: it names a task, it does
    /// not touch the turn in flight, and it has no ladder behind it. There is
    /// nothing to escalate *to* — a signal reaches the process group, which is
    /// the CLI and every other task it is minding, so the only honest failure
    /// mode for a request about one child is to have not worked.
    ///
    /// Fire-and-forget on purpose. The answer to "did it stop" is not the
    /// `control_response` — that says the request was accepted, and claude
    /// answers success even for a task it cannot find — it is the
    /// `background_tasks_changed` snapshot that arrives a moment later without
    /// the task in it. That snapshot is the only thing this server ever believed
    /// about which tasks are alive, so a stop that did not take shows up as a
    /// row that is simply still there, which is the truth.
    ///
    /// Nothing reads a resident's stdin before `system:init`, so a request sent
    /// during the launch window would be dropped on the floor. A background task
    /// cannot exist before the process that published it is up, so this is a
    /// guard against races rather than a case with a user behind it.
    async fn stop_task(&mut self, task_id: &str) {
        if !self.ready {
            tracing::info!(session = %self.id, task = %task_id, "cannot stop a task before claude says it is listening");
            return;
        }
        let request = serde_json::json!({
            "type": "control_request",
            "request_id": format!("stop-{}", new_uuid()),
            "request": {"subtype": "stop_task", "task_id": task_id},
        })
        .to_string();
        self.recorder
            .lifecycle(Stage::Interrupt, Some("asking claude to stop one background task"));
        self.recorder.stdin(&request);
        if let Err(error) = self.link.send_line(&request).await {
            tracing::warn!(session = %self.id, task = %task_id, %error, "cannot ask claude to stop that task");
        }
    }

    /// Signal claude's process group, one rung harder than last time.
    ///
    /// Reached only when the polite request produced no `result`, which for a
    /// healthy CLI it always does — so every rung below is a report that
    /// something is wedged, and is logged as one.
    fn escalate(&mut self, turn: u64, rung: u8) {
        // A rung that comes due after its turn has ended is exactly what
        // success looks like: the request worked and the timer lost the race.
        if self.turn.as_ref().map(|turn| turn.seq) != Some(turn) {
            return;
        }
        let (name, signal) = match rung {
            1 => ("SIGINT", libc::SIGINT),
            2 => ("SIGTERM", libc::SIGTERM),
            _ => ("SIGKILL", libc::SIGKILL),
        };
        tracing::warn!(session = %self.id, rung, signal = name, "claude did not stop when asked; escalating");
        self.recorder.lifecycle_with(Stage::Escalate, Some(rung), None, Some(name));
        if let Some(turn) = &mut self.turn {
            turn.ladder = Some(rung);
        }
        self.link.kill_claude(signal);
        if rung < 3 {
            self.arm(turn, rung + 1);
        }
    }

    /// Schedule the next rung, without holding the driver up for it.
    fn arm(&self, turn: u64, rung: u8) {
        let tx = self.tx.clone();
        let step = self.registry.interrupt_step;
        tokio::spawn(async move {
            tokio::time::sleep(step).await;
            let _ = tx.send(Cmd::Escalate { turn, rung });
        });
    }

    // --------------------------------------------------------------- queue

    /// Start the next message, if the process is free and fits it.
    ///
    /// One message per call, which is what the state machine says: the turn
    /// this opens ends with a `result`, and that is what calls this again.
    async fn pump(&mut self) {
        // Deliberately not gated on `ready`: stdin is buffered, so a message
        // written before `init` is simply the first thing claude reads — and
        // waiting for `init` to write it deadlocks a fresh session outright,
        // because claude 2.1.231 given `--session-id` under stream-json does
        // not say `init` until it has read a first message. (Its hooks run and
        // report; the init line just never comes.) remote-hand mints the id,
        // so delivery must not wait for a line that is itself waiting for
        // delivery.
        // A question open is a turn in flight, whatever `turn` says: claude is
        // blocked mid-turn, and the respawn a queued settings change could
        // bring would end it — and take the question with it.
        if self.turn.is_some() || self.replaying || !self.asked.is_empty() {
            return;
        }
        // Steered messages first, and not as a policy choice: they are already
        // in claude's stdin, so the CLI will answer them before anything
        // written afterwards whatever this driver believes. Reading them in a
        // different order would only make the bookkeeping disagree with the
        // process.
        //
        // Read for folds once more first: a message claude took into the turn
        // that just ended has no turn of its own coming, and the poll may not
        // have come round since it landed. See `absorb`.
        if !self.delivered_ahead.is_empty() {
            self.absorb().await;
        }
        if let Some(delivered) = self.delivered_ahead.pop_front() {
            self.set_awaiting(false);
            // Published before the phase moves, exactly as the ordinary path
            // below does it: a reader that caught the two in the other order
            // would see a session `working` on a message it was still counting
            // as queued, and report the queue as one longer than it is.
            self.publish();
            self.start_delivered_turn(&delivered);
            return;
        }
        // A limit with nowhere to go: the account that refused gets nothing
        // more until its limit lifts or somebody acts. Checked after the
        // steered messages, because those are in claude's stdin already and
        // run whatever this driver decides.
        if self.hold.is_some() {
            self.set_awaiting(false);
            self.set_phase(Phase::Idle);
            return;
        }
        let Some(mut next) = self.queue.pop_front() else {
            self.set_awaiting(false);
            self.set_phase(Phase::Idle);
            return;
        };
        // The other quiet point: an idle session about to start a turn. A
        // reading another session made since may have taken this account to the
        // edge; a move now retargets this message too, and sends it down the
        // respawn path below instead of into the account that is running out.
        if self.registry.rotate_ahead.is_some() && next.fingerprint.accepted_by(self.live.as_ref()) {
            self.queue.push_front(next);
            self.rotate_ahead().await;
            let Some(again) = self.queue.pop_front() else { return };
            next = again;
        }
        if !next.fingerprint.accepted_by(self.live.as_ref()) {
            // A journalled message for the account the session is bound to now
            // gets a recipe for it; see [`Driver::recipe_for_binding`].
            if next.recipe.is_none() {
                if let Some(recipe) = self.recipe_for_binding(&next).await {
                    next.fingerprint = recipe.fingerprint.clone();
                    next.recipe = Some(recipe);
                }
            }
            let Some(wanted) = next.recipe.clone() else {
                // Reachable only where the process has been *replaced* under a
                // message the journal kept: adoption keeps one because it fitted
                // what the record said, or because the record said nothing, and
                // a recovery respawn can then bring back a process neither
                // description covers. Such a message can be neither delivered
                // nor spawned for, and would park the queue for ever, so it is
                // failed out loud instead.
                //
                // Out loud, and *specific*: the environment this message would
                // need carries an account token and was never journalled, so
                // there is nothing to rebuild a process from and nothing the
                // driver can do about it. What it can do is say which setting
                // is in the way, so the answer — send it again — is obvious
                // rather than a mystery about a server restart. `live` is known
                // here by construction: an unknown one accepts every message.
                let differs = self
                    .live
                    .as_ref()
                    .map(|live| next.fingerprint.differences(live))
                    .unwrap_or_else(|| "different settings".to_string());
                tracing::error!(session = %self.id, %differs, "a re-delivered message no longer fits the process running this session");
                self.receipt(&next, ReceiptStatus::Failed);
                self.fail_message(&format!(
                    "this message needs {differs}, which means a new process, and the settings to start one \
                     did not survive the server restart — send it again"
                ));
                self.publish();
                Box::pin(self.pump()).await;
                return;
            };
            // A settings change is a *restart*, and a restart kills whatever the
            // process was minding in the background — the exact thing residents
            // exist to stop happening. So the message waits at the head of the
            // queue instead, and the session says why: the alternative is
            // silently trading somebody's half-hour background job for a model
            // switch they could have made a minute later.
            if self.minding_background().await && !self.waited_long_enough() {
                tracing::info!(
                    session = %self.id,
                    tasks = self.background.len(),
                    "a settings change is waiting for claude's background work to finish"
                );
                self.queue.push_front(next);
                self.set_awaiting(true);
                self.set_phase(Phase::Idle);
                self.publish();
                return;
            }
            // A rotation's continuation that waited for background work, and
            // is going now with some of it still running, says what it cut.
            let next = self.account_for_interruptions(next).await;
            // The replacement says nothing until it reads something — the same
            // CLI behaviour the gate above documents — so the head of the queue
            // goes straight in rather than waiting on its `init`.
            if self.respawn_for(next, &wanted).await {
                Box::pin(self.pump()).await;
            }
            return;
        }
        self.take_settings(&next);
        self.set_awaiting(false);
        self.publish();
        self.start_turn(next).await;
    }

    /// Take what the message going in says about the process it is going into.
    ///
    /// Two things a driver can be missing, both taken **once** and only from a
    /// message that fits the running process — which is to say from a message
    /// describing a process equivalent to it:
    ///
    /// * the **recipe**, which is what a re-attached driver has no other way to
    ///   get: the journal remembers the settings but cannot hold an environment
    ///   (it carries the account's token), so until a message brings one the
    ///   recovery ladder has nothing to spawn;
    /// * the **fingerprint**, when the record this driver adopted predates the
    ///   field. Unknown accepted this message rather than judging it, and the
    ///   message is now the best evidence there is about what the process will
    ///   answer to — so it becomes the answer, and every message after it is
    ///   compared properly again.
    ///
    /// Both from the same message, deliberately: taken apart they could describe
    /// two different processes, and the pair is what a respawn re-creates.
    ///
    /// The heal is written to disk immediately. An unknown that stayed in the
    /// record would come back unknown on the next restart, and the next server
    /// would have to guess all over again — which is the whole failure this is
    /// here to end.
    fn take_settings(&mut self, pending: &Pending) {
        // A message read back out of the journal has neither, so there is
        // nothing to take: it is delivered on the strength of the vetting the
        // *previous* server did, and says nothing this one did not already know.
        let Some(recipe) = pending.recipe.as_ref() else { return };
        if self.recipe.is_none() {
            self.recipe = Some(recipe.clone());
        }
        if self.live.is_some() {
            return;
        }
        tracing::info!(
            session = %self.id,
            "adopted a resident whose settings were never recorded; taking them from the message going into it"
        );
        // `Settle` because this is the attach still working out what it
        // inherited, a message later than the replay: the record named no
        // settings, and this is the line that says which ones it now has.
        self.recorder.lifecycle(
            Stage::Settle,
            Some("the record of this resident named no settings; taking the ones of the message going into it"),
        );
        self.live = Some(pending.fingerprint.clone());
        self.note_fingerprint();
    }

    /// Whether claude is still looking after work a turn left running.
    ///
    /// Two signals, and the first is the real one. `system:background_tasks_changed`
    /// carries claude's whole live task list every time it changes (2.1.231), so
    /// the set is authoritative whenever it has been seen.
    ///
    /// The second is a mtime on the subagent transcripts, and it is here for the
    /// case the first cannot cover: the shim's replay ring can overflow while no
    /// server is attached, and a `task_started` lost in that gap would leave the
    /// set saying "nothing running" about an agent that very much is. Reading a
    /// directory per settings change is cheap, and the window is short enough
    /// that the false positives it also produces — a turn that ran a *foreground*
    /// subagent looks the same — cost seconds rather than minutes.
    async fn minding_background(&self) -> bool {
        !self.background.is_empty() || self.subagents_recently_active().await
    }

    /// Whether anything claude's background work writes to was written to just now.
    ///
    /// The corroborating half of [`Driver::minding_background`], and the only
    /// evidence available about work whose events did not survive. Deliberately
    /// short-windowed, because it also fires for a *foreground* subagent and
    /// every second of the window is a second a settings change waits for
    /// nothing.
    ///
    /// Two places, because background work comes in two shapes and neither one
    /// covers the other. A background **Agent** streams a transcript into the
    /// session's `subagents/` directory; a background **Bash** writes no
    /// transcript at all, only its `<task-id>.output` under
    /// [`InjectionRegistry::task_root`]. A replayed set of the second kind has
    /// no witness in the first place, so both are read and the newest wins.
    async fn subagents_recently_active(&self) -> bool {
        let Some(file) = crate::sessions::claude::find_claude_session_file(&self.registry.paths, &self.id).await else {
            return false;
        };
        let mut newest = newest_write(&crate::sessions::transcript::subagent_dir(&file)).await;
        if let Some(tasks) = self.task_output_dir().await {
            // `Option<SystemTime>` orders `None` below every `Some`, so this is
            // "the newer of the two, or whichever one exists".
            newest = newest.max(newest_write(&tasks).await);
        }
        newest
            .and_then(|at| SystemTime::now().duration_since(at).ok())
            .is_some_and(|quiet| quiet < SUBAGENT_QUIET)
    }

    /// Where this session's background tasks write their output, if it exists yet.
    ///
    /// Found rather than built: the directory sits under a slug of the working
    /// directory, and that slug mangles separators and non-ASCII in ways not
    /// worth reproducing from this side. The session id below it is unique, so
    /// one shallow scan of the root finds it without knowing the encoding.
    ///
    /// Not cached, because the answer changes from `None` to a path the first
    /// time the session runs a background task, and a cache primed before that
    /// would be wrong for the life of the process. The scan is a `read_dir` of a
    /// directory with one entry per project and a `stat` each, and it only runs
    /// on the rare paths — a replay boundary, a parked settings change, an idle
    /// sweep — never per line.
    async fn task_output_dir(&self) -> Option<PathBuf> {
        let mut entries = tokio::fs::read_dir(&self.registry.task_root).await.ok()?;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let candidate = entry.path().join(&self.id).join("tasks");
            if tokio::fs::metadata(&candidate).await.is_ok() {
                return Some(candidate);
            }
        }
        None
    }

    /// Throw away background tasks the replay invented.
    ///
    /// A replayed `task_started` whose `task_finished` fell off the end of the
    /// shim's ring — or was written while no server was attached, or belonged
    /// to a process that died mid-task — comes back as a task that will never
    /// finish. Nothing ever removes it: `task_finished` is the only thing that
    /// does, and it already went past. The set is then permanently non-empty,
    /// [`Driver::minding_background`] is permanently true, and the first
    /// message that asks for a different model is parked behind it for the life
    /// of the process. Since a parked session sits at `Phase::Idle`, Stop
    /// refuses it too: there is no way out at all.
    ///
    /// So a replayed set is *provisional* until something outside the replay
    /// agrees with it. The witness is a fresh file: claude streams a background
    /// agent's transcript and a background command's output while they run, so
    /// real background work at the moment of an attach has written something in
    /// the last few seconds — see [`Driver::subagents_recently_active`] for the
    /// two places it writes. Nothing on disk means nothing is running, and the
    /// set is dropped.
    ///
    /// Only ever runs at the replay boundary. A `task_started` seen live is
    /// claude telling this driver, in this process' lifetime, that a task
    /// began; there is nothing provisional about that one.
    async fn settle_background(&mut self) {
        if self.background.is_empty() {
            return;
        }
        if self.subagents_recently_active().await {
            tracing::info!(
                session = %self.id,
                tasks = self.background.len(),
                "the replay's background tasks are corroborated by fresh subagent transcripts; keeping them"
            );
            return;
        }
        let ghosts = self.background.len();
        self.background.clear();
        tracing::warn!(
            session = %self.id,
            ghosts,
            "the replay left background tasks with no sign of life on disk; dropping them rather than holding the queue for ever"
        );
        self.recorder.lifecycle(
            Stage::Settle,
            Some(&format!(
                "dropped {ghosts} background task(s) the replay reconstructed: nothing has been written to a subagent transcript or a task output file in {SUBAGENT_QUIET:?}"
            )),
        );
        self.publish();
    }

    /// Write one message to claude's stdin and open a turn for it.
    async fn start_turn(&mut self, pending: Pending) {
        let message = user_line(&pending.text);
        let turn = Turn {
            run: self.open_turn(),
            at: self.link.at(),
            seq: self.seq,
            ladder: None,
            provisional: false,
            rotation_attempt: pending.rotation_attempt,
        };
        // Written *before* the prompt, in one rewrite that both takes this
        // message out of the queue and says where its turn began. A crash in the
        // microsecond between this and the write below leaves a mark for a turn
        // that never started, which the next boot reads as in flight and the
        // process' own silence never contradicts — recoverable with Stop. The
        // other order sends the same message twice.
        self.note_progress(Some(turn.at));
        self.recorder.stdin(&message);
        self.recorder.lifecycle(Stage::TurnOpen, None);
        if let Err(error) = self.link.send_line(&message).await {
            // The socket is gone, which means the shim is: the recv loop is
            // about to see the same thing, so the turn is failed here and the
            // driver is left to wind up normally.
            self.receipt(&pending, ReceiptStatus::Failed);
            self.close_turn(&turn, InjectionRunState::Failed, Some(format!("cannot reach claude: {error}")));
            return;
        }
        self.receipt(&pending, ReceiptStatus::Sent);
        self.touch();
        self.set_phase(Phase::Working);
        self.turn = Some(turn);
    }

    /// Open a turn for a message claude was already handed.
    ///
    /// The steered half of [`Driver::start_turn`], and the difference is the
    /// whole of it: nothing is written. The prompt went into claude's stdin
    /// when the steer was accepted, possibly minutes ago and certainly during
    /// somebody else's turn; writing it again here would run it twice.
    ///
    /// # Why this turn is only provisional
    ///
    /// Because the CLI has two answers to a message written mid-turn and does
    /// not say which it gave. Against claude 2.1.231, both are real:
    ///
    /// * it **buffers** the line and runs it as the next turn, with a
    ///   `system:init` of its own and its own `result` — the case this driver
    ///   was written for, and the one a probe that writes between turns sees;
    /// * it **injects** the line into the turn already running, at the next
    ///   tool boundary, and the model answers both messages in one reply under
    ///   the *original* turn's `result`. Nothing on stdout marks it: there is
    ///   no `user` echo, no second `result`, nothing. Only the transcript,
    ///   which this driver does not read, records a `queued_command`.
    ///
    /// A turn opened unconditionally for the second case is a turn nothing will
    /// ever end — the session shows `working` for ever and everything behind it
    /// in the queue is held there — which is exactly what happened to a live
    /// session and is why this function no longer simply asserts a turn exists.
    ///
    /// So it opens one and gives it [`InjectionRegistry::steer_settle`] to
    /// prove itself. The journal mark is the one piece of bookkeeping that
    /// waits for that proof: a mark is what the next boot reconstructs a turn
    /// in flight from, and a mark for a turn that was folded into a previous
    /// one would resurrect the ghost across a restart.
    fn start_delivered_turn(&mut self, pending: &Pending) {
        let turn = Turn {
            run: self.open_turn(),
            // Where the *turn* starts, not where the prompt was written: from
            // this offset on, everything claude says belongs to this message,
            // which is what a replay after a restart has to reconstruct.
            at: self.link.at(),
            seq: self.seq,
            ladder: None,
            provisional: true,
            rotation_attempt: pending.rotation_attempt,
        };
        tracing::info!(session = %self.id, qid = %pending.id, "a steered message reached the front of the queue");
        // Deliberately `None`: the queue moved and has to be written down, but
        // the turn is not yet one a restart should believe in.
        self.note_progress(None);
        self.recorder.lifecycle(Stage::TurnOpen, Some("a steered message claude already had"));
        self.set_phase(Phase::Working);
        let seq = turn.seq;
        self.turn = Some(turn);
        self.arm_settle(seq, self.registry.steer_settle);
    }

    /// Schedule the moment a provisional turn is judged, without waiting for it.
    ///
    /// The sibling of [`Driver::arm`], and separate from it for the same reason
    /// the two commands are separate: this one must not touch a turn that has
    /// since been replaced, and the only thing that can say so is the sequence
    /// number it carries.
    fn arm_settle(&self, turn: u64, after: Duration) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let _ = tx.send(Cmd::Settle { turn });
        });
    }

    /// Decide what became of a steered message whose turn has proved nothing.
    ///
    /// Two conditions, and both are needed:
    ///
    /// * **no `init`** for this turn. That is the positive signal — a buffered
    ///   message is always answered with one — and it is what
    ///   [`Driver::on_init`] clears `provisional` on;
    /// * **silence** for the whole window. Belt to the init's braces: anything
    ///   at all on stdout, background chatter included, pushes the decision out
    ///   rather than risking a verdict against a CLI that is mid-sentence.
    ///
    /// When both hold, the message was answered inside the turn it was handed
    /// to. The turn is closed `Ok` — the work really did happen, and the person
    /// who sent it has already read the answer — and the queue behind it moves.
    /// The alternative, which is what shipped and what a live session died of,
    /// is a turn nobody will ever close.
    ///
    /// A Stop pressed inside the window is not overridden: the ladder is armed
    /// against this turn's sequence number, so closing the turn here disarms it
    /// on its own — see [`Driver::escalate`] — and a process that was going to
    /// be signalled for a turn that had already finished is left alone instead.
    async fn settle_delivered(&mut self, seq: u64) {
        let Some(turn) = self.turn.as_ref().filter(|turn| turn.seq == seq && turn.provisional) else {
            return; // ended, replaced, or claude said `init` and proved it real
        };
        let window = self.registry.steer_settle;
        let quiet = self.heard.elapsed();
        if quiet < window {
            // claude is talking; ask again once the silence would be long
            // enough. Re-armed rather than looped so the driver stays free to
            // answer the very lines this is waiting on, and floored so that a
            // stream arriving exactly at the edge of the window cannot turn
            // this into a spin.
            self.arm_settle(turn.seq, (window - quiet).max(SETTLE_RECHECK));
            return;
        }
        let turn = self.turn.take().expect("checked just above");
        tracing::info!(
            session = %self.id,
            ?window,
            "claude never opened a turn for the steered message; it was answered inside the turn it was handed to"
        );
        self.recorder.lifecycle(
            Stage::Absorb,
            Some(&format!(
                "no turn of its own after {window:?}: the steered message was folded into the turn it was handed to"
            )),
        );
        self.close_turn(&turn, InjectionRunState::Ok, None);
        self.pump().await;
    }

    /// Take out of [`Driver::delivered_ahead`] every steered message claude has
    /// already folded into the turn in flight, by reading the transcript for it.
    ///
    /// # Why
    ///
    /// The CLI does not always hold a `user` line written mid-turn until the
    /// turn ends. At the next tool boundary it may fold the line into the turn
    /// that is running instead: the model reads it in its next request and
    /// answers it inside the same turn, and the transcript records it — as a
    /// `queued_command` attachment, where it landed — while stdout says nothing
    /// at all. A driver that only found out at the turn's end kept the message
    /// queued for the whole rest of that turn: the row said "delivered, waiting
    /// to start" beside a Stop hint, and the row, the list badge and the
    /// "1 message queued" line all counted it, while the transcript above them
    /// showed the agent already answering it.
    ///
    /// # How
    ///
    /// The transcript is read from the moment of the steer on — only the bytes
    /// past [`AbsorbAnchor::offset`], only lines that name a queued command, and
    /// only the folds of a person's message (see
    /// [`crate::sessions::transcript::absorbed_prompt`]). A fold whose text is a
    /// steered message's takes the oldest such message out. Its turn will never
    /// open, nothing about it is queued any more, and it is not this driver's to
    /// deliver again: it is in the conversation now rather than in the pipe, so
    /// a process that dies after this must not have [`Driver::requeue_delivered`]
    /// hand it to the replacement a second time.
    ///
    /// A message the CLI *buffers* for a turn of its own leaves no such entry —
    /// its record is an ordinary `user` line, written as that turn starts — so
    /// it stays exactly where it was, for [`Driver::start_delivered_turn`] and
    /// the settle to decide as before. A fold that matches nothing outstanding
    /// is left alone, and a fold this misses (inherited across a restart, say)
    /// is still caught by the settle once the turn ends: this only ever makes
    /// the queue right sooner.
    ///
    /// Returns whether anything was taken.
    async fn absorb(&mut self) -> bool {
        if self.delivered_ahead.is_empty() {
            self.absorb.anchor = None;
            return false;
        }
        // History still being replayed may change what is outstanding.
        if self.replaying {
            return false;
        }
        if !self.absorb_anchored() {
            // Nothing this driver steered is in the file yet: the messages were
            // inherited from the last server, or the session moved to a fork.
            // Starting from the end can only miss a fold, and a missed fold is
            // the settle's to find at the turn's end.
            self.anchor_absorb().await;
            return false;
        }
        let anchor = self.absorb.anchor.as_mut().expect("checked just above");
        if anchor.file.is_none() {
            // There was no transcript when the watch began, so all of it is newer.
            anchor.file = crate::sessions::claude::find_claude_session_file(&self.registry.paths, &self.id).await;
        }
        let Some(path) = anchor.file.clone() else { return false };
        let (lines, offset) = appended_lines(&path, anchor.offset, b"\"queued_command\"").await;
        anchor.offset = offset;
        let mut taken = Vec::new();
        for text in lines.iter().filter_map(crate::sessions::transcript::absorbed_prompt) {
            if let Some(at) = self.delivered_ahead.iter().position(|pending| same_prompt(&pending.text, &text)) {
                taken.push(self.delivered_ahead.remove(at).expect("the index came from this queue"));
            }
        }
        if self.delivered_ahead.is_empty() {
            self.absorb.anchor = None;
        }
        if taken.is_empty() {
            return false;
        }
        for pending in &taken {
            tracing::info!(session = %self.id, qid = %pending.id, "claude folded a steered message into the turn in flight");
            self.recorder.lifecycle(
                Stage::Absorb,
                Some(&format!(
                    "{} was folded into the turn in flight; the transcript holds it: {}",
                    pending.id,
                    clip(&pending.text)
                )),
            );
        }
        self.absorb.unannounced = true;
        self.publish();
        // The list's `+1` and queued chips count it too.
        self.registry.refresh_list();
        // Out of the journal too, or the next server inherits a steered message
        // that is already in the conversation and opens a turn for it.
        self.note_progress(self.turn.as_ref().filter(|turn| !turn.provisional).map(|turn| turn.at));
        true
    }

    /// Whether the transcript watch is reading this session's current file.
    fn absorb_anchored(&self) -> bool {
        self.absorb.anchor.as_ref().is_some_and(|anchor| anchor.session == self.id)
    }

    /// Watch the transcript from where it ends now.
    ///
    /// Called before a steer writes anything, so the fold that message may
    /// become is always past the anchor. The end of the file may be the middle
    /// of a line claude is still writing; that fragment fails to parse and is
    /// stepped over, and it cannot be the fold of a message not yet written.
    async fn anchor_absorb(&mut self) {
        let file = crate::sessions::claude::find_claude_session_file(&self.registry.paths, &self.id).await;
        let offset = match &file {
            Some(path) => tokio::fs::metadata(path).await.map(|meta| meta.len()).unwrap_or(0),
            None => 0,
        };
        self.absorb.anchor = Some(AbsorbAnchor {
            session: self.id.clone(),
            file,
            offset,
        });
    }

    /// [`Cmd::Absorb`]: look now, and keep looking while anything is outstanding.
    ///
    /// The feed's nudge is the moment to poke it, too: it has just sent the
    /// transcript frame that shows the folded message, so the state frame that
    /// takes the message off the queue follows right behind — not seconds
    /// later, when the screen would show it twice, and not ahead of it, when it
    /// would briefly be in neither place. The driver's own poll can read the
    /// file before the feed has, so it leaves the poke to that nudge, or to the
    /// feed's own tick for a session nobody has open.
    async fn on_absorb(&mut self, nudged: bool) {
        if !nudged {
            self.absorb.armed = false;
        }
        self.absorb().await;
        if nudged && std::mem::take(&mut self.absorb.unannounced) {
            self.registry.poke_feed(Source::Claude, &self.id);
        }
        self.arm_absorb();
    }

    /// Schedule the next look, unless one is coming or nothing is outstanding.
    fn arm_absorb(&mut self) {
        if self.absorb.armed || self.delivered_ahead.is_empty() {
            return;
        }
        self.absorb.armed = true;
        let tx = self.tx.clone();
        let after = self.registry.absorb_poll;
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let _ = tx.send(Cmd::Absorb { nudged: false });
        });
    }

    /// Put `next` back at the head of the queue and replace the process with
    /// one it fits.
    ///
    /// The one place a settings change is carried out, shared by the two ways
    /// of asking for it: [`Driver::pump`] reaching a head the running process
    /// cannot answer, and [`Driver::restart_for_steer`] being told to stop
    /// waiting. Both have to leave the same state behind — the message first in
    /// the queue, no park outstanding, and the mirror saying so before a
    /// restart nobody is watching takes seconds — and two copies of that is how
    /// they would stop agreeing.
    ///
    /// Returns whether there is a process to pump the queue into: a respawn
    /// that failed set `fatal` and closed the link, and the driver's own loop
    /// is what winds the session up, with nothing written at it.
    async fn respawn_for(&mut self, next: Pending, wanted: &Recipe) -> bool {
        self.queue.push_front(next);
        // Whatever this session was waiting for, it is not waiting any more.
        self.set_awaiting(false);
        self.publish();
        self.respawn(wanted).await;
        self.fatal.is_none()
    }

    /// Replace the running process with one started under `recipe`.
    ///
    /// Only ever from idle, so the polite shutdown is the whole of it: stdin
    /// closes, claude exits zero, and the kill below is for a CLI that will
    /// not. A replacement that cannot be started leaves the closed link in
    /// place on purpose — the driver's own `recv` sees the end immediately and
    /// winds the session up through the one path that does that properly.
    async fn respawn(&mut self, recipe: &Recipe) {
        tracing::info!(session = %self.id, "the session's settings changed; restarting claude");
        self.recorder.lifecycle_with(
            Stage::Respawn,
            None,
            Some(Selector::Resume.name()),
            Some("the settings this session's next message needs differ from the ones it is running"),
        );
        if self.link.close_stdin().await.is_err() {
            self.link.kill_claude(libc::SIGTERM);
        }
        let deadline = tokio::time::Instant::now() + CLOSE_GRACE;
        loop {
            match tokio::time::timeout_at(deadline, self.link.recv()).await {
                Ok(Some(_)) => continue,
                Ok(None) => break,
                Err(_) => {
                    tracing::warn!(session = %self.id, "claude did not exit when its stdin closed; killing it");
                    self.link.kill_claude(libc::SIGKILL);
                    break;
                }
            }
        }
        // Always a resume from here: the process being replaced has written the
        // transcript this one has to continue, whatever selector started it.
        match self.relink(recipe, Selector::Resume).await {
            Ok(()) => {}
            Err(error) => {
                tracing::error!(session = %self.id, %error, "cannot restart claude under the new settings");
                self.fatal = Some(error.to_string());
            }
        }
    }

    /// Put a freshly spawned process behind this driver, retiring the old one.
    ///
    /// Shared by the settings respawn and by the recovery ladder, which is what
    /// keeps the two from drifting: both have to retire the outgoing journal
    /// record, drop the outgoing sidecars, and go back to `Starting` with
    /// nothing writable until the replacement says `init`.
    async fn relink(&mut self, recipe: &Recipe, selector: Selector) -> Result<(), InjectError> {
        // Nothing is steered into a process this driver is replacing: whatever
        // was written to the outgoing one's stdin is going nowhere, and the
        // record about to be written names the *new* process. Keeping that
        // invariant here rather than at each caller is what lets the journal's
        // `steered` flag mean one thing — "the process this record names has
        // this line in its stdin" — which is the whole basis on which an attach
        // declines to send it again.
        self.requeue_delivered();
        // Nothing replaces a process with a question open — see `pump` — so
        // this is the recovery ladder's, already done; and if not, the process
        // that asked is about to be gone either way.
        self.close_questions(QuestionState::Expired, "the process that asked it is being replaced");
        // Whatever the outgoing process was thinking, it is not thinking it
        // any more.
        self.draft.end();
        // The backlog moves to the new record with the process it is waiting
        // for, so a crash between the two spawns leaves the messages attached to
        // whichever record survives rather than to neither.
        let queued = self.backlog(Pending::recorded);
        let spawned = self
            .registry
            .spawn_link(&self.id, recipe, selector, Some(&self.record), &queued, &self.recorder)
            .await?;
        let _ = std::fs::remove_file(self.sidecars.exit());
        let _ = std::fs::remove_file(self.sidecars.stderr());
        self.sidecars = spawned.sidecars;
        self.link = spawned.link;
        self.record = spawned.record;
        // A queue held because one account refused is not held against a
        // process on another; one restarted on the same account (the recovery
        // ladder) still is, and the new record has to say so.
        if self
            .hold
            .as_ref()
            .is_some_and(|hold| recipe.fingerprint.account.as_deref() != Some(hold.account.as_str()))
        {
            self.hold = None;
        }
        if self.hold.is_some() {
            self.registry.journal.mark_hold(&self.record, self.hold.clone());
        }
        // Known again, whatever it was before: this driver started the process
        // itself, and the record it just wrote says the same thing.
        self.live = Some(recipe.fingerprint.clone());
        self.recipe = Some(recipe.clone());
        self.ready = false;
        // A replacement is a fresh process with nothing behind it: its shim has
        // written no bytes, so there is no history and nothing to suppress.
        self.replaying = false;
        self.replay_open = false;
        self.replay_marked = false;
        self.replay_gapped = false;
        self.pending_turn = None;
        self.idle_since = None;
        // The background tasks belonged to the process that is gone, and they
        // went with it — claude does not hand a subagent to its successor. A
        // fresh process only announces the list when it *changes*, so a set
        // carried across would never be contradicted and would hold every later
        // settings change hostage to work that stopped existing here.
        if !self.background.is_empty() {
            tracing::info!(
                session = %self.id,
                tasks = self.background.len(),
                "the replaced process took its background tasks with it"
            );
            self.background.clear();
        }
        // The mirror first, the phase second: the phase pokes the feed, and
        // the frame that poke produces should already have the cleared tasks
        // and the moved backlog in it.
        self.publish();
        self.set_phase(Phase::Starting);
        self.publish();
        Ok(())
    }

    // ------------------------------------------------------------ watchdog

    /// Notice a turn that has gone quiet, and do nothing about it.
    ///
    /// The watchdog this replaces ended a run that made no progress for twenty
    /// minutes, which against a resident would mean killing every session that
    /// is merely between messages. It was also wrong for the case it was
    /// written for: a build, a test suite or a background agent can legitimately
    /// produce nothing for an hour, and a watchdog that kills them destroys work
    /// nobody can get back.
    ///
    /// So this marks and never kills, and only while a turn is in flight — a
    /// process that has not reached `system:init` counts, because a launch that
    /// never finishes is exactly as stuck. The mark is what a Stop button is
    /// offered beside; the decision stays the user's.
    async fn watchdog(&mut self) {
        // A queue held after a limit asks its question again every tick: has
        // the limit lifted, or has an account freed up?
        if self.hold.is_some() {
            self.recheck_hold().await;
        }
        // Waiting on the user is neither a stall nor a ghost, and certainly not
        // idle. claude says nothing at all while a question is open — that is
        // what blocked means — and the transcript, asked whether a turn is in
        // flight, reads a tail ending on `AskUserQuestion` as "waiting for
        // input". Both are right, and either one acted on would throw away the
        // turn the question belongs to and push the queue into the middle of
        // it. The user takes as long as they take.
        if !self.asked.is_empty() {
            self.idle_since = None;
            return;
        }
        if self.turn.is_none() && self.ready {
            // Idle: no stall clock runs at all. Two other things do, and both
            // are about a process with nothing to do rather than a turn with
            // nothing to show.
            if self.awaiting_background {
                // The parked settings change. Woken by an event when there is
                // one, but the mtime signal in `minding_background` has no event
                // behind it, so something has to come back and ask again.
                self.pump().await;
            }
            self.idle_shutdown().await;
            return;
        }
        self.idle_since = None;
        if self.heard.elapsed() < self.registry.stall_after {
            return;
        }
        // The transcript is the other half of "quiet": a turn writing tool
        // results is making progress even while the stream-json side is silent.
        if let Some(progress) = super::transcript_progress(Source::Claude, &self.registry.paths, &self.id).await {
            if progress > self.heard {
                self.heard = progress;
            }
        }
        if self.heard.elapsed() < self.registry.stall_after {
            return;
        }
        // Quiet on both channels for long enough to be worth a second opinion,
        // and the transcript is the one witness that can give it. A turn it says
        // is not happening is a turn this driver reconstructed from something
        // that was not work — a `system` line after the last `result`, a replay
        // gap, a stale turn mark — and such a turn never ends on its own: the
        // `result` that would close it was written before this server attached.
        // The queue behind it would wait for ever, which is the failure this is
        // here to stop; getting it wrong the other way costs one turn re-read as
        // idle after ten minutes of total silence.
        if self.turn.is_some() && self.transcript_is_between_turns().await {
            self.discard_ghost_turn().await;
            return;
        }
        let mut shared = self.shared.lock().expect("resident mirror poisoned");
        if !shared.stalled {
            shared.stalled = true;
            drop(shared);
            self.recorder
                .lifecycle(Stage::Stalled, Some(&format!("nothing heard for {:?}", self.heard.elapsed())));
            tracing::warn!(
                session = %self.id,
                quiet_for = ?self.heard.elapsed(),
                "claude has produced nothing for a long time; marking the turn stalled and leaving it alone"
            );
        }
    }

    /// Retire a resident that has had nothing to do for long enough.
    ///
    /// Off by default, and deliberately so: a resident is a whole `claude`, and
    /// a whole `claude` is three or four hundred megabytes of resident memory
    /// per session that is doing nothing at all. On a machine with a handful of
    /// sessions that is the price of the thing working — the next message goes
    /// into a process that is already warm, and nothing in the background is
    /// ever cut off. On a machine with fifty it is not, and `DROPPEN_RESIDENT_IDLE_MS`
    /// is how that machine says so.
    ///
    /// What makes it safe is that nothing is lost by it. `close_stdin` is the
    /// polite end — claude writes its last line and exits zero — the claim and
    /// the journal record go with the driver, and the next message spawns a
    /// `--resume` that picks the conversation up where it was. What it must not
    /// do is end a process that is minding background work, which is the same
    /// guard the settings respawn takes.
    async fn idle_shutdown(&mut self) {
        let Some(after) = self.registry.resident_idle else { return };
        // A steered message is outstanding work even though no turn is open for
        // it: ending the process would take it with it. So is a held queue's
        // promise to continue the failed message.
        if !self.queue.is_empty() || !self.delivered_ahead.is_empty() || self.hold.is_some() || !self.asked.is_empty() {
            self.idle_since = None;
            return;
        }
        let since = *self.idle_since.get_or_insert_with(Instant::now);
        if since.elapsed() < after {
            return;
        }
        if self.minding_background().await {
            // Not idle at all: the turn ended, the work did not. The clock is
            // restarted rather than merely skipped, so a session is given the
            // full quiet period *after* its background work finishes.
            self.idle_since = None;
            return;
        }
        tracing::info!(session = %self.id, idle_for = ?since.elapsed(), "retiring an idle resident claude");
        // Marked before the stdin goes, because the exit it causes arrives on
        // the same loop that decides whether an exit is a crash.
        self.retiring = true;
        if self.link.close_stdin().await.is_err() {
            self.link.kill_claude(libc::SIGTERM);
        }
    }

    // ------------------------------------------------------------ recovery

    /// The process died. Bring it back, or say why it is not coming back.
    ///
    /// `true` means a replacement is behind the link and the driver's loop
    /// should carry on; `false` means the session is over and [`Driver::finish`]
    /// takes it from here.
    ///
    /// Only *unexpected* deaths are recovered, and everything else in this file
    /// exists to make that distinction cheap:
    ///
    /// * an **idle** resident that went away has lost nothing — that is the
    ///   whole difference between a process ending and a turn ending — so it is
    ///   cleaned up in silence and the next message starts a fresh one;
    /// * a resident this server **stopped on purpose** — the interrupt ladder's
    ///   last rung, the idle shutdown — is not a crash, and restarting it would
    ///   undo the thing that was asked for;
    /// * a **settings respawn** that could not start its replacement has already
    ///   said so in `fatal`, and trying the same spawn again would only fail
    ///   again.
    async fn recover(&mut self) -> bool {
        // A question is a promise from *this* process: the turn it blocks is
        // the only place an answer could go, and `--resume` closes the call as
        // interrupted and never asks it again. So it expires the moment the
        // process does, whatever happens next — and stays answerable, as an
        // ordinary message to whatever comes back.
        self.close_questions(QuestionState::Expired, "the process that asked it is gone");
        // Its draft goes with it: a process that is gone is not thinking.
        self.draft.end();
        // Before anything decides whether there is work left: a message steered
        // into the process that just died went into a pipe nothing will ever
        // read, so it is undelivered again — and it has to be back in the queue
        // *now*, both because the emptiness test below is the test for "is
        // there anything to recover for", and because everything downstream
        // (the relink's journal entry, the failure report if the breaker trips)
        // reads the queue and not this.
        self.requeue_delivered();
        let in_flight = self.turn.as_ref().is_some_and(|turn| turn.ladder.is_none());
        // `recovering` keeps an episode alive between rungs: a replacement that
        // dies *before saying it is listening* has no turn and usually no queue,
        // and without the flag it would read as an idle death — ending the
        // episode one rung in, with the fork never tried.
        if self.retiring || self.fatal.is_some() || (!in_flight && self.queue.is_empty() && !self.recovering) {
            return false;
        }
        let Some(recipe) = self.recipe.clone().or_else(|| self.queue.front().and_then(|next| next.recipe.clone())) else {
            // A resident inherited through an attach, with nothing queued behind
            // it: the journal knows what settings it ran under but not the
            // environment it ran with, and inventing one would start claude with
            // no credentials against somebody's repository.
            tracing::warn!(session = %self.id, "cannot recover a re-attached resident: nothing has described how to start one");
            self.fatal = Some("claude died and this server does not know how to start it again".to_string());
            return false;
        };

        // The breaker counts deaths, not attempts, and over a window rather than
        // in a row: a session that dies four times in five minutes is not going
        // to be fixed by a fifth restart, and one that dies once an hour is a
        // session that has been running for hours.
        let now = Instant::now();
        self.deaths.retain(|at| now.duration_since(*at) < RECOVERY_WINDOW);
        self.deaths.push_back(now);
        if self.deaths.len() >= RECOVERY_STRIKES {
            tracing::error!(session = %self.id, deaths = self.deaths.len(), "a resident claude is in a crash loop; giving up");
            self.fatal = Some(format!(
                "claude died {} times in {} minutes and could not be recovered (tried --resume, then --resume --fork-session)",
                self.deaths.len(),
                RECOVERY_WINDOW.as_secs() / 60,
            ));
            return false;
        }
        let selector = match self.rung {
            0 => Selector::Resume,
            1 => Selector::Fork,
            _ => {
                tracing::error!(session = %self.id, "a resident claude could not be resumed or forked; giving up");
                self.fatal = Some("claude died and neither --resume nor --resume --fork-session could bring the session back".to_string());
                return false;
            }
        };
        // The rung only advances for a process that died *before* saying it was
        // listening, because that is what makes the resume itself the suspect. A
        // process that reached `init` and then fell over has a healthy resume
        // path and a different problem, and forking its session would answer the
        // wrong question — see [`Driver::on_init`], which resets this.
        if !self.ready {
            self.rung = self.rung.saturating_add(1);
        }
        let ladder = &self.registry.recovery_backoff;
        let backoff = ladder[(self.deaths.len() - 1).min(ladder.len() - 1)];
        tracing::warn!(
            session = %self.id,
            ?selector,
            ?backoff,
            deaths = self.deaths.len(),
            "a resident claude died unexpectedly; recovering"
        );
        // The death's own reason, which is what makes a recording of a crash
        // loop readable: three `recover` lines all saying "exited with code 91"
        // is a different bug from three saying different things.
        self.recorder
            .lifecycle_with(Stage::Recover, None, Some(selector.name()), Some(&self.death_reason()));
        tokio::time::sleep(backoff).await;

        // The turn that was in flight is failed either way. Its work is in the
        // transcript up to the point claude stopped, and re-sending the prompt
        // into the resumed session would run the half of it that already
        // happened a second time — writing the same files, sending the same
        // mail. The queue is different: those messages were never delivered at
        // all, and now there is a session to deliver them to.
        if let Some(turn) = self.turn.take() {
            let reason = self.death_reason();
            self.close_turn(
                &turn,
                InjectionRunState::Failed,
                Some(format!("{reason}\nthe session was recovered; this turn was not")),
            );
        }
        if let Err(error) = self.relink(&recipe, selector).await {
            tracing::error!(session = %self.id, %error, ?selector, "cannot start a replacement for a resident that died");
            self.fatal = Some(format!("claude died and could not be restarted: {error}"));
            return false;
        }
        self.recovering = true;
        self.publish();
        // The replacement says nothing until it reads something (see `pump`),
        // so the queue this recovery exists to deliver is pumped at it now
        // rather than waited on.
        self.pump().await;
        true
    }

    /// Put steered messages back in the queue, for a process that cannot run them.
    ///
    /// The one thing that undoes a steer, and the only safe one: the write went
    /// into a pipe whose reader is gone, so the message provably never reached
    /// claude and re-delivering it runs it for the first time. Back at the
    /// *head*, in order, because that is where they were — ahead of everything
    /// still queued.
    ///
    /// Note what this is not: a message whose turn had already opened is not
    /// here to be requeued. It left `delivered_ahead` when its turn started,
    /// and a turn that died is failed rather than retried — see
    /// [`Driver::recover`].
    fn requeue_delivered(&mut self) {
        if self.delivered_ahead.is_empty() {
            return;
        }
        tracing::warn!(
            session = %self.id,
            steered = self.delivered_ahead.len(),
            "messages steered into a process that died were never read; they go back to the queue"
        );
        while let Some(pending) = self.delivered_ahead.pop_back() {
            self.queue.push_front(pending);
        }
        self.publish();
    }

    /// What the tombstone and the stderr tail say about the exit just seen.
    fn death_reason(&self) -> String {
        let reason = match self.sidecars.read_tombstone() {
            Some(shim::Exit { signal: Some(signal), .. }) => format!("claude was {}", super::signal_name(signal).to_lowercase()),
            Some(shim::Exit { status: Some(code), .. }) => format!("claude exited with code {code}"),
            _ => "claude exited for an unknown reason".to_string(),
        };
        let tail = self
            .sidecars
            .read_stderr()
            .map(|stderr| self.registry.redact(&last_lines(&stderr, STDERR_TAIL_LINES)))
            .unwrap_or_default();
        if tail.is_empty() {
            reason
        } else {
            format!("{reason}\n{tail}")
        }
    }

    // --------------------------------------------------------------- exits

    /// The process is gone; say what that means for the turn it was running.
    ///
    /// The distinction here is the point of the whole module: a resident that
    /// dies between messages costs nothing and is cleaned up in silence, while
    /// one that dies *during* a turn has lost work somebody is waiting for and
    /// owes them the reason. The reason outlives the process because the shim
    /// wrote it down first — the tombstone and the stderr tail are already on
    /// disk by the time `recv` returns `None`.
    async fn finish(mut self) {
        // Belt and braces: every path here but one comes through
        // [`Driver::recover`], which has already done this. The exception is a
        // driver whose inbox closed, and a message reported as undelivered is
        // the right answer for that too. The same for the questions.
        self.requeue_delivered();
        self.close_questions(QuestionState::Expired, "the process that asked it is gone");
        self.draft.end();
        let gave_up = self.fatal.is_some();
        let detailed = match self.fatal.take() {
            Some(fatal) => fatal,
            None => self.death_reason(),
        };
        let reason = detailed.lines().next().unwrap_or_default().to_string();
        // The tombstone and the stderr tail, whole: this is the last line of the
        // recording and the only one that can say why a session stopped. The
        // recorder itself lives until the end of this function, so the transcript
        // tailer gets a final sweep after it.
        self.recorder.lifecycle(Stage::Exit, Some(&detailed));

        if let Some(turn) = self.turn.take() {
            let interrupted = turn.ladder.is_some();
            tracing::warn!(session = %self.id, %reason, interrupted, "a resident claude died with a turn in flight");
            let error = if interrupted {
                "interrupted from Droppen".to_string()
            } else {
                detailed.clone()
            };
            self.close_turn(&turn, InjectionRunState::Failed, Some(error));
        } else {
            tracing::info!(session = %self.id, %reason, queued = self.queue.len(), "a resident claude ended");
            // A resident that died before its replay could say what it was in
            // the middle of leaves the run adoption published and no turn to
            // close it with. It is closed here instead, and only when nothing is
            // queued: the block below opens a run of its own for those, and two
            // failures for one death is one notification too many.
            if self.queue.is_empty() {
                if let Some(run) = self.published_run() {
                    let turn = Turn {
                        run,
                        at: 0,
                        seq: self.seq,
                        ladder: None,
                        provisional: false,
                        rotation_attempt: 0,
                    };
                    self.close_turn(&turn, InjectionRunState::Failed, Some(detailed.clone()));
                } else if gave_up {
                    // A verdict like the breaker's arrives with no turn to carry
                    // it — the turn died rungs ago — and it must not evaporate:
                    // this is the one line telling the user their session is not
                    // coming back on its own. A run is opened for the purpose.
                    let turn = Turn {
                        run: self.open_turn(),
                        at: self.link.at(),
                        seq: self.seq,
                        ladder: None,
                        provisional: false,
                        rotation_attempt: 0,
                    };
                    self.close_turn(&turn, InjectionRunState::Failed, Some(detailed.clone()));
                }
            }
        }

        // The queue only reaches here once [`Driver::recover`] has run out of
        // ways to give it a session to arrive in. Saying so is the last thing
        // this driver can usefully do: the messages are gone, and the person who
        // sent them has to hear it from somewhere.
        if !self.queue.is_empty() {
            let dropped = self.queue.len();
            for pending in &self.queue {
                self.receipt(pending, ReceiptStatus::Failed);
            }
            let turn = Turn {
                run: self.open_turn(),
                at: self.link.at(),
                seq: self.seq,
                ladder: None,
                provisional: false,
                rotation_attempt: 0,
            };
            self.close_turn(
                &turn,
                InjectionRunState::Failed,
                Some(format!("{detailed}\n{dropped} queued message(s) were not delivered")),
            );
        }

        // Its own entry, and only if it is still its own: `adopt` may have moved
        // the key, and a racing send may already have put a replacement there.
        {
            let mut residents = self.registry.residents.lock().expect("residents poisoned");
            if residents.get(&self.id).is_some_and(|handle| handle.tx.same_channel(&self.tx)) {
                residents.remove(&self.id);
            }
        }
        self.registry.journal.remove(&self.record);
        // The claim goes last, and outside every other lock: dropping it
        // unlinks a file, and until it is gone this session is still ours.
        drop(self.claim.take());
        let _ = std::fs::remove_file(self.sidecars.exit());
        let _ = std::fs::remove_file(self.sidecars.stderr());
    }

    // --------------------------------------------------------- bookkeeping

    /// Put a `Running` injection in the registry for the turn about to start.
    fn open_turn(&mut self) -> Arc<Run> {
        self.seq += 1;
        let run = running_turn(&self.id);
        self.registry
            .runs
            .lock()
            .expect("injections poisoned")
            .insert(self.registry.key_of(Source::Claude, &self.id), run.clone());
        run
    }

    /// The run to fail when a launch is stopped before it ever started a turn.
    ///
    /// [`InjectionRegistry::start_resident`] put one there the moment the send
    /// was accepted, so that a session waiting on a cold CLI does not read as
    /// idle. That is the run whoever pressed Stop is looking at, so it is the
    /// one that has to be closed — opening a second would leave the first
    /// running for ever.
    fn launching_turn(&mut self) -> Arc<Run> {
        match self.published_run() {
            Some(run) => {
                self.seq += 1;
                run
            }
            None => self.open_turn(),
        }
    }

    /// The `Running` injection this session already has, if it has one.
    ///
    /// Always one this driver is responsible for: a session with a resident is
    /// claimed, and nothing else may put a run against a claimed session. It is
    /// either the one the send path published while the CLI was starting, or the
    /// one adoption published while the replay was still arriving.
    fn published_run(&self) -> Option<Arc<Run>> {
        let runs = self.registry.runs.lock().expect("injections poisoned");
        runs.get(&self.registry.key_of(Source::Claude, &self.id))
            .filter(|run| run.state.lock().expect("run state poisoned").state == InjectionRunState::Running)
            .cloned()
    }

    /// Take back a run that was published for a turn there turns out not to be.
    ///
    /// Removed rather than closed: a turn that ended while this server was down
    /// needs no explanation, and a failure reported on every restart would be
    /// noise about something that went fine.
    fn withdraw_published_run(&self) {
        self.registry
            .runs
            .lock()
            .expect("injections poisoned")
            .remove(&self.registry.key_of(Source::Claude, &self.id));
    }

    /// Write a turn's outcome, tell the phone, and go back to idle.
    fn close_turn(&mut self, turn: &Turn, outcome: InjectionRunState, error: Option<String>) {
        // Every way a turn can end comes through here, which is why the journal
        // is cleared here rather than beside the `result` that usually does it:
        // an interrupted turn, a failed write and a dead process all have to
        // leave the record saying the same thing. The recording is written at
        // the same point and for the same reason, and so is the draft closed —
        // a no-op after the `result` that has already done it.
        self.note_progress(None);
        self.draft.end();
        self.recorder.lifecycle(
            Stage::Result,
            Some(&match (&error, outcome) {
                (Some(error), _) => format!("{outcome:?}: {error}"),
                (None, outcome) => format!("{outcome:?}"),
            }),
        );
        {
            let mut state = turn.run.state.lock().expect("run state poisoned");
            state.state = outcome;
            state.error = error.clone();
            state.finished_at = Some(iso8601(std::time::SystemTime::now()));
        }
        let _ = turn.run.finished.send(true);
        // The phase goes back to idle *before* the notification, and the order is
        // the whole point: [`crate::push`] now reads the driver's own account of
        // the session, and a snapshot taken a line earlier would still say
        // `Working` — it would report the turn that just ended as still in
        // flight. After this line what is left in the snapshot is only what is
        // genuinely outstanding: a message queued behind this turn, or background
        // work claude is still minding.
        self.set_phase(Phase::Idle);
        // Taken here rather than inside the spawned task. `on_result` pumps the
        // next queued message the moment this returns, so a snapshot read
        // asynchronously would be a coin toss between this turn and the next.
        let driver = self.registry.driver_states();
        // Once per turn, where the old model could only manage once per process.
        // A resident answering ten messages now sends ten notifications, and the
        // throttle in [`crate::push`] is what keeps that from being ten lines in
        // the notification shade.
        crate::push::notify_turn(self.registry.paths.clone(), Source::Claude, self.id.clone(), outcome, error, driver);
    }

    /// Record that claude is alive and talking.
    fn touch(&mut self) {
        self.heard = Instant::now();
        self.shared.lock().expect("resident mirror poisoned").stalled = false;
    }

    fn set_phase(&self, phase: Phase) {
        let moved = {
            let mut shared = self.shared.lock().expect("resident mirror poisoned");
            let moved = shared.phase != phase;
            shared.phase = phase;
            if phase != Phase::Working {
                shared.stalled = false;
            }
            // The turn's clock: started by whatever leaves `Idle`, and not restarted
            // by a recovery that passes back through `Starting` mid-turn.
            match phase {
                Phase::Idle => shared.turn_started_at = None,
                Phase::Starting | Phase::Working => {
                    if shared.turn_started_at.is_none() {
                        shared.turn_started_at = Some(iso8601(SystemTime::now()));
                    }
                }
            }
            moved
        };
        // A turn opening or closing is the one change a page following the
        // session most wants to see, and no file says it: the transcript has
        // usually stopped moving by the time the turn is over. Left to the
        // feed's poll, "working" outlived the turn by up to two seconds.
        //
        // The feed reads the state on its own task, possibly before a caller
        // that moves the phase and then publishes the queue has got to the
        // second half — so the publish that follows pokes once more, and the
        // frame that settles is the whole of the change rather than half of it.
        if moved {
            self.phase_moved.store(true, std::sync::atomic::Ordering::Relaxed);
            self.registry.poke_feed(Source::Claude, &self.id);
        }
    }

    /// Say — or stop saying — that a settings change is waiting on background work.
    fn set_awaiting(&mut self, awaiting: bool) {
        self.awaiting_background = awaiting;
        if !awaiting {
            self.awaiting_since = None;
        }
        self.shared.lock().expect("resident mirror poisoned").awaiting_background = awaiting;
    }

    /// Whether a parked settings change has waited past the point of politeness.
    ///
    /// The park has no other way out. A parked driver sits at `Phase::Idle`, so
    /// [`InjectionRegistry::interrupt_resident`] refuses it — there is no turn
    /// to stop — and the message at the head of the queue is neither delivered
    /// nor failed. Every release depends on the background set emptying, and
    /// the whole reason [`Driver::settle_background`] exists is that the set
    /// can be wrong about work that is not there.
    ///
    /// So there is a ceiling. Reaching it restarts the process, which does end
    /// whatever claude was genuinely minding — the trade this makes deliberately,
    /// because the alternative is a session that answers nothing, ever, and
    /// cannot be told to stop. It is loud in the log and on the row
    /// (`awaitingBackground`) for the whole wait, and `DROPPEN_AWAIT_BACKGROUND_MS=0`
    /// removes it for an installation that would rather wait for ever.
    fn waited_long_enough(&mut self) -> bool {
        let Some(limit) = self.registry.await_background else { return false };
        let since = *self.awaiting_since.get_or_insert_with(Instant::now);
        if since.elapsed() < limit {
            return false;
        }
        tracing::warn!(
            session = %self.id,
            tasks = self.background.len(),
            waited = ?since.elapsed(),
            "a settings change has waited for background work past the limit; restarting claude anyway"
        );
        self.recorder.lifecycle(
            Stage::Respawn,
            Some(&format!(
                "a settings change waited {:?} for {} background task(s) and gave up",
                since.elapsed(),
                self.background.len()
            )),
        );
        self.awaiting_since = None;
        true
    }

    fn publish(&self) {
        {
            let mut shared = self.shared.lock().expect("resident mirror poisoned");
            shared.queued = self.delivered_ahead.len() + self.queue.len();
            shared.queued_messages = self.backlog(Pending::preview);
            shared.background = self.background.len();
            shared.tasks = self.background.wire();
            shared.question = self.asked.first().map(|question| question.request_id.clone());
        }
        // Only after a phase change: see `set_phase`. Everything else published
        // here waits for the feed's own poll, as it always has.
        if self.phase_moved.swap(false, std::sync::atomic::Ordering::Relaxed) {
            self.registry.poke_feed(Source::Claude, &self.id);
        }
    }

    /// Everything outstanding, in the order it will run.
    ///
    /// One function for the two shapes a backlog is written in — the mirror's
    /// and the journal's — because the *order* is the part that has to agree,
    /// and a second hand-rolled chain is how it would stop agreeing. Steered
    /// first, then the queue, each entry told which it was.
    fn backlog<T>(&self, shape: impl Fn(&Pending, bool) -> T) -> Vec<T> {
        self.delivered_ahead
            .iter()
            .map(|pending| shape(pending, true))
            .chain(self.queue.iter().map(|pending| shape(pending, false)))
            .collect()
    }

    /// Write down what this resident is in the middle of.
    ///
    /// The disk counterpart of [`Driver::publish`], and the difference between
    /// them is who is asking: `publish` answers this server's own readers, while
    /// this answers the *next* server's. Called wherever the queue or the turn
    /// moves, and it restates both every time rather than tracking which changed
    /// — the record is small, and a rule of "always both" cannot leave one of
    /// them stale.
    fn note_progress(&self, turn_at: Option<u64>) {
        self.registry
            .journal
            .mark_progress(&self.record, turn_at, &self.backlog(Pending::recorded));
    }

    /// Write down what the process this driver is holding is running.
    ///
    /// Only ever a correction, and only from the two places that can know
    /// better than the spawn did: the settings taken off the first message an
    /// adopted resident is given, and the model name `system:init` resolved. A
    /// driver that still knows nothing writes nothing rather than writing the
    /// default, which is the whole distinction this change turns on.
    fn note_fingerprint(&self) {
        let Some(live) = self.live.as_ref() else { return };
        self.registry.journal.mark_fingerprint(&self.record, live);
    }

    /// Tell the receipt store what became of a message — a no-op for one no
    /// client named. See [`super::receipts`].
    fn receipt(&self, pending: &Pending, status: ReceiptStatus) {
        self.registry.note_receipt(Source::Claude, &self.id, &pending.id, status);
    }

    /// Report a message that will never be delivered, as its own failed turn.
    ///
    /// The message is already out of the queue; what is left is telling the
    /// person who sent it, and a failed injection is the only channel this
    /// session has for that.
    fn fail_message(&mut self, why: &str) {
        let turn = Turn {
            run: self.open_turn(),
            at: self.link.at(),
            seq: self.seq,
            ladder: None,
            provisional: false,
            rotation_attempt: 0,
        };
        self.close_turn(&turn, InjectionRunState::Failed, Some(why.to_string()));
    }
}

/// The most recent write anywhere in a directory: its own mtime, and its
/// entries', whichever is newest. `None` when the directory is not there.
///
/// The entries are the whole point, and reading the directory alone was a bug.
/// A directory's mtime moves when an entry is created, renamed or removed — and
/// never when one is *written to*. Claude creates a background agent's
/// transcript when the agent starts and then streams into it for as long as the
/// agent runs, so a directory whose last agent was dispatched a minute ago looks
/// untouched while every agent in it is still working. Measured on a live
/// session with two running background agents: the directory 133 seconds stale,
/// the transcripts inside it written that same second — a probe reading only the
/// directory calls that session quiet, and drops the tasks that prove it is not.
///
/// One shallow pass, no recursion: both directories this is asked about hold
/// files, not trees.
async fn newest_write(dir: &Path) -> Option<SystemTime> {
    let mut newest = tokio::fs::metadata(dir).await.ok()?.modified().ok();
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return newest;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(at) = entry.metadata().await.and_then(|meta| meta.modified()) else {
            continue;
        };
        if newest.is_none_or(|seen| at > seen) {
            newest = Some(at);
        }
    }
    newest
}

/// One prompt, in the shape claude reads it: a single line of stream-json.
///
/// Shared by the pump and by a steer, which is the point: the two write the
/// same bytes at different moments, and a second copy of this would be a second
/// chance to write a message claude parses differently depending on how it got
/// there.
fn user_line(text: &str) -> String {
    serde_json::json!({
        "type": "user",
        "message": {"role": "user", "content": [{"type": "text", "text": text}]},
    })
    .to_string()
}

/// A `Running` injection for one turn of `id`.
///
/// Built here rather than in [`Driver::open_turn`] because the send path needs
/// the same thing before there is a driver to ask: see
/// [`InjectionRegistry::start_resident`].
fn running_turn(id: &str) -> Arc<Run> {
    let (finished, _) = tokio::sync::watch::channel(false);
    Arc::new(Run {
        state: Mutex::new(RunState {
            state: InjectionRunState::Running,
            error: None,
            interrupted: false,
            started_at: iso8601(std::time::SystemTime::now()),
            finished_at: None,
            // The pid is the resident's, and the resident is not this turn's to
            // end: Stop goes through the driver's ladder, which is the reason
            // nothing here is allowed to signal it directly.
            pid: None,
            pid_token: None,
            last_progress: Instant::now(),
            idle_killed: false,
            current_id: id.to_string(),
            // The journal entry belongs to the *process*, not to the turn, so a
            // turn ending must never retire it.
            run_id: String::new(),
            lock: None,
        }),
        finished,
    })
}

/// Which of the three futures woke the driver.
enum Wake {
    Output(Option<Output>),
    Command(Option<Cmd>),
    Tick,
}

/// The handful of things the driver acts on, out of everything claude writes.
#[derive(Debug, PartialEq, Eq)]
enum Event {
    /// `system:init` — the process is listening, under this session id, and
    /// running this model (the resolved id, not whatever alias started it).
    Init { session: Option<String>, model: Option<String> },
    /// The turn is over.
    Result {
        is_error: bool,
        subtype: String,
        detail: Option<String>,
        /// `api_error_status`: the HTTP status behind a failed turn, when the
        /// CLI says — 429 beside "You've hit your session limit".
        api_status: Option<u16>,
    },
    /// A `rate_limit_event` that is a refusal rather than a reading.
    ///
    /// claude writes one before nearly every turn, almost always saying
    /// `status: allowed`; those are [`Event::UsageReading`]. Only one whose
    /// status is anything else — `rejected`, in every recording so far — is
    /// this, so the limit log can record it (see [`Driver::note_rate_limit`]).
    /// Its windows are a reading all the same, and are filed as one. It changes
    /// nothing about the turn.
    RateLimit { info: Value },
    /// A `rate_limit_event` that is not a refusal but carries `unifiedWindows`:
    /// a quota reading of the login this process runs on, and nothing else (see
    /// [`Driver::note_usage`]). One without windows says nothing and stays
    /// [`Event::Other`].
    UsageReading { info: Value },
    /// `system:background_tasks_changed` — claude's whole live task list.
    BackgroundTasks { tasks: Vec<TaskSeen> },
    /// `system:task_started` — one more task is running.
    TaskStarted { task: TaskSeen },
    /// `system:task_progress` — how a running task is getting on.
    TaskProgress { progress: TaskProgress },
    /// `system:task_notification` — one task has reached a terminal state.
    TaskFinished { id: String },
    /// The session's own thread said something.
    ///
    /// An `assistant` or `user` event that is this conversation's rather than a
    /// background subagent's, forwarded. The only line on the whole stream that
    /// means "the main agent is working right now", and therefore the only one
    /// that can open a turn nobody wrote a prompt for — see
    /// [`Driver::on_speech`].
    Speech,
    /// An answer to something we asked over stdin.
    ControlResponse,
    /// `control_request` / `can_use_tool`: claude wants to run a tool and is
    /// asking whether it may. A question for the user when the tool is
    /// `AskUserQuestion`; a permission prompt, refused, otherwise. Only ever
    /// written by a process started with `--permission-prompt-tool stdio`.
    Permission(Box<PermissionRequest>),
    /// Any other `control_request`. Nothing this server registers for exists,
    /// so each is answered with an error rather than left hanging.
    ControlRequest { request_id: Option<String> },
    /// `control_cancel_request`: claude withdrew a request it had made — what
    /// an interrupt does to a question still waiting for its answer.
    ControlCancel { request_id: String },
    /// A `system` line this driver does not act on: `commands_changed`,
    /// `task_updated`, `thinking_tokens`, and whatever the next CLI release
    /// adds beside them. (`thinking_tokens` does move the draft's counter —
    /// see [`super::drafts`] — which is a reading, not an act: the turn, the
    /// queue and the replay are exactly as they would be without it.)
    ///
    /// Told apart from [`Event::Other`] because the *replay* has to treat the
    /// two differently, and reading them alike is what produced ghost turns. The
    /// live rule has always been that a `system` line may not open a turn — see
    /// [`Driver::on_speech`] — because the CLI publishes them while the session
    /// sits idle: a background task reporting progress, or the command list
    /// changing when a hook edits it. The replay had no such rule, so a single
    /// `system` line written after the last `result` made the whole history read
    /// as ending mid-turn, and the turn that reconstructed could never end.
    Housekeeping,
    /// A line that is JSON but says nothing this driver acts on.
    Other,
    /// A line that is not JSON at all.
    Unparseable,
}

/// Read one stdout line, refusing to be surprised by it.
///
/// The discipline this encodes is the expensive lesson from every other client
/// of this stream: **parse tolerantly, never drop**. The CLI adds types, adds
/// subtypes, and changes the shape of fields between patch releases, so a reader
/// that validated strictly would keep meeting perfectly good sessions it decided
/// were broken. Everything unrecognised is [`Event::Other`] and everything
/// unparseable is [`Event::Unparseable`]; neither is an error, and neither ends
/// a turn. Only the events above are acted on, and each is read with
/// `unwrap_or` defaults rather than a schema.
///
/// # The background-task events, as claude 2.1.231 actually writes them
///
/// Observed rather than documented — the CLI's own docs describe none of this —
/// by running a real `claude -p --input-format stream-json` and asking it for a
/// `run_in_background` Bash command and a `run_in_background` Agent:
///
/// ```text
/// {"type":"system","subtype":"background_tasks_changed",
///  "tasks":[{"task_id":"b9xgm1tdf","task_type":"local_bash",
///            "description":"Background sleep and file write task"}],…}
/// {"type":"system","subtype":"task_started","task_id":"b9xgm1tdf",
///  "tool_use_id":"toolu_01UC…","description":"…","task_type":"local_bash",…}
/// {"type":"system","subtype":"task_progress","task_id":"…","usage":{…},
///  "last_tool_name":"Bash",…}
/// {"type":"system","subtype":"task_updated","task_id":"b9xgm1tdf",
///  "patch":{"status":"completed","end_time":1787983045880},…}
/// {"type":"system","subtype":"task_notification","task_id":"b9xgm1tdf",
///  "status":"completed","output_file":"…/tasks/b9xgm1tdf.output",
///  "summary":"Background command \"…\" completed (exit code 0)",…}
/// ```
///
/// Three things about that shape decided the code above.
///
/// 1. `background_tasks_changed` is a **snapshot, not a delta** — the whole live
///    list, republished on every change, including the empty list when the last
///    task ends. So it needs no history to be read correctly, which is exactly
///    what a cursor-zero replay needs.
/// 2. A background **Agent** emits `task_started` too, with
///    `task_type:"local_agent"` and a `subagent_type`, and a background Bash the
///    *subagent* then starts emits its own with `owned_by_subagent:true`. (2.1.226
///    spawned background agents through a text channel with no `task_started` at
///    all; that is no longer true on 2.1.231, and the
///    subagent-directory signal in [`Driver::minding_background`] is what covers
///    the difference either way.)
/// 3. A task finishing **opens a turn of its own**: `task_notification` is
///    followed by a fresh `system:init`, assistant messages and a `result`, with
///    no user message anywhere. That is why a `result` with no turn in flight is
///    a debug line rather than a failure, and why `init` does not mean "a turn
///    started" — see [`Driver::replay`]. It is also why [`Event::Speech`]
///    exists: that turn is real work, and the only thing on the stream that
///    says it is running is the main thread's own `assistant` and `user` events
///    — see [`Driver::on_speech`].
fn classify(line: &str) -> Event {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Event::Unparseable;
    };
    classify_value(&value)
}

/// [`classify`], for a line the caller has already parsed.
fn classify_value(value: &Value) -> Event {
    let subtype = value.get("subtype").and_then(Value::as_str);
    match value.get("type").and_then(Value::as_str) {
        Some("system") if subtype == Some("init") => Event::Init {
            // A missing id is not a reason to refuse the init: the process is
            // listening either way, and the id we asked for stays in force.
            session: value.get("session_id").and_then(Value::as_str).map(str::to_owned),
            model: value.get("model").and_then(Value::as_str).map(str::to_owned),
        },
        // The authoritative one. Every change to what claude is running in the
        // background republishes the whole list, so this needs no history to be
        // read correctly — which is what makes it survive a replay.
        Some("system") if subtype == Some("background_tasks_changed") => Event::BackgroundTasks {
            tasks: value
                .get("tasks")
                .and_then(Value::as_array)
                .map(|tasks| tasks.iter().filter_map(TaskSeen::parse).collect())
                .unwrap_or_default(),
        },
        Some("system") if subtype == Some("task_started") => match TaskSeen::parse(value) {
            Some(task) => Event::TaskStarted { task },
            None => Event::Housekeeping,
        },
        // Not a change to the list, so it never opens or closes anything: it
        // only fills in what a task that is already running is doing.
        Some("system") if subtype == Some("task_progress") => match TaskProgress::parse(value) {
            Some(progress) => Event::TaskProgress { progress },
            None => Event::Housekeeping,
        },
        // `task_notification` is the terminal one and `task_updated` is not:
        // the latter carries a `patch` whose `status` may be anything, and
        // reading a status field that is documented nowhere as the end of a
        // task would be guessing. A notification is only ever sent once the
        // task is over.
        Some("system") if subtype == Some("task_notification") => match value.get("task_id").and_then(Value::as_str) {
            Some(id) => Event::TaskFinished { id: id.to_string() },
            None => Event::Housekeeping,
        },
        // The main thread talking, told from a subagent's by one field.
        //
        // A background `Task` runs its own agent and the CLI republishes that
        // agent's whole stream on the parent's stdout, tagged with the
        // `parent_tool_use_id` of the tool use that started it. The session's
        // own messages carry `null`, or nothing at all. Counted off a real
        // session that ran four background agents (`3c002577`, 2026-08-30):
        // 588 forwarded events, every one of them carrying an id, and every
        // main-thread event without one.
        //
        // `system` lines are deliberately not here even though they too carry a
        // null id: `task_progress` and `task_updated` are published for a
        // background task while the session sits idle, and reading those as the
        // main thread working would leave every session that ever ran one
        // looking permanently busy.
        // Every other `system` subtype, named as what it is rather than left in
        // with the unknowns: the CLI writes these while a session is idle as
        // readily as while it is working, so neither the live path nor the
        // replay may read one as the main thread doing something.
        Some("system") => Event::Housekeeping,
        Some("assistant") | Some("user") if value.get("parent_tool_use_id").is_none_or(Value::is_null) => Event::Speech,
        Some("result") => Event::Result {
            is_error: value.get("is_error").and_then(Value::as_bool).unwrap_or(false),
            subtype: value.get("subtype").and_then(Value::as_str).unwrap_or("an error").to_string(),
            detail: value.get("result").and_then(Value::as_str).map(str::to_owned),
            api_status: value
                .get("api_error_status")
                .and_then(Value::as_u64)
                .and_then(|status| u16::try_from(status).ok()),
        },
        // `allowed` and `allowed_warning` are readings, and a line with no
        // status is not a refusal either; either one is worth something only
        // for the windows it carries.
        Some("rate_limit_event")
            if value
                .pointer("/rate_limit_info/status")
                .and_then(Value::as_str)
                .is_some_and(|status| !status.starts_with("allowed")) =>
        {
            Event::RateLimit {
                info: value["rate_limit_info"].clone(),
            }
        }
        Some("rate_limit_event") if value.pointer("/rate_limit_info/unifiedWindows").is_some_and(Value::is_object) => Event::UsageReading {
            info: value["rate_limit_info"].clone(),
        },
        Some("control_response") => Event::ControlResponse,
        // claude asking *us*. The shapes are [`super::questions`]'s to read,
        // and the only place they are spelled.
        Some("control_request") => match PermissionRequest::parse(value) {
            Some(request) => Event::Permission(Box::new(request)),
            None => Event::ControlRequest {
                request_id: value.get("request_id").and_then(Value::as_str).map(str::to_owned),
            },
        },
        Some("control_cancel_request") => match value.get("request_id").and_then(Value::as_str) {
            Some(id) => Event::ControlCancel { request_id: id.to_string() },
            None => Event::Other,
        },
        _ => Event::Other,
    }
}

/// The `tool_use` ids a stdout line carries results for, if it is a `user`
/// event at all. Parsed again rather than carried on [`Event::Speech`]: it is
/// only ever asked while a question or a refusal is outstanding, which is a
/// handful of lines in a session's life.
fn answered_in(line: &str) -> Vec<String> {
    serde_json::from_str::<Value>(line)
        .map(|value| questions::answered_tool_uses(&value))
        .unwrap_or_default()
}

/// Which Claude root a spawn with this environment runs in.
///
/// Only a spawn against this instance's own root sets `CLAUDE_CONFIG_DIR` —
/// see `owned_claude_env` — and the server's own value never leaks through,
/// because `claude_env` strips every inherited `CLAUDE_*`. So its absence is the
/// user's own `~/.claude`, and its presence is the managed root.
fn recipe_root(env: &Env) -> crate::claude_roots::ClaudeRoot {
    if env.contains_key("CLAUDE_CONFIG_DIR") {
        crate::claude_roots::ClaudeRoot::Managed
    } else {
        crate::claude_roots::ClaudeRoot::Local
    }
}

/// The last `count` lines of a blob, for a failure message.
fn last_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

/// A line, shortened enough to log.
fn clip(line: &str) -> String {
    clip_to(line, LOG_LINE_CAP)
}

/// The first `cap` characters of `text`, with an ellipsis where it was cut.
///
/// Counted in characters rather than bytes: the cheap byte slice would panic on
/// a multi-byte boundary, and a prompt is the most likely place in this server
/// to meet one.
fn clip_to(text: &str, cap: usize) -> String {
    if text.chars().count() <= cap {
        return text.to_string();
    }
    text.chars().take(cap).chain(['…']).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::Path;

    use crate::instance::InstanceDirs;
    use crate::sessions::Paths;

    /// The id every fake claude in this module answers under.
    const SESSION: &str = "11111111-1111-4111-8111-111111111111";

    /// The id a `--fork-session` answers under instead.
    const FORKED: &str = "22222222-2222-4222-8222-222222222222";

    // ------------------------------------------------------------ pure parts

    /// What a test's tracing subscriber wrote.
    #[derive(Clone, Default)]
    struct Logs(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Logs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Logs {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    /// A send that waits more than two seconds for its resident to journal
    /// the message is logged — when it passes the mark, and with the total
    /// when it ends — so a slow acknowledgement can be told apart from a slow
    /// network in production. A quick one is not; and the wait itself is the
    /// one it always was (it still ends only when the driver says so).
    #[tokio::test(start_paused = true)]
    async fn a_send_that_waits_long_for_the_journal_is_logged_and_a_quick_one_is_not() {
        let logs = Logs::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt().with_writer(move || writer.clone()).with_ansi(false).finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let (tx, rx) = tokio::sync::oneshot::channel();
        tx.send(()).unwrap();
        assert!(wait_recorded("s-quick", false, rx).await.is_ok());
        assert_eq!(logs.text(), "", "a quick journal is not logged");

        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(3_500)).await;
            let _ = tx.send(());
        });
        let started = tokio::time::Instant::now();
        assert!(wait_recorded("s-slow", true, rx).await.is_ok());
        assert_eq!(started.elapsed(), Duration::from_millis(3_500), "it waited for the driver, as before");
        let text = logs.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(
            lines[0].contains("still waiting") && lines[0].contains("session=\"s-slow\"") && lines[0].contains("waited_ms=2000"),
            "{text}"
        );
        assert!(
            lines[1].contains("waited_ms=3500") && lines[1].contains("recorded=true") && lines[1].contains("queued=true"),
            "{text}"
        );

        // A driver that ends without recording it: the caller still hears so.
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            drop(tx);
        });
        assert!(wait_recorded("s-gone", false, rx).await.is_err());
        assert!(logs.text().contains("recorded=false"));
    }

    /// Editing an account's credential has to reach the process already
    /// running under it.
    ///
    /// The bug: an env account whose `ANTHROPIC_BASE_URL` had one path segment
    /// too many was fixed, every new session worked, and the session the user
    /// was sitting in went on failing — because the resident had been started
    /// with the old block, and the account id, which was all a fingerprint
    /// knew, had not changed.
    #[test]
    fn the_same_account_with_a_different_credential_is_a_different_process() {
        let under = |credential: &str| Fingerprint {
            account: Some("acc-1".into()),
            credential: Some(credential.into()),
            ..Default::default()
        };
        let live = under("hash-before-the-fix");
        assert!(live.satisfied_by(&live), "the same credential is the same login");
        assert!(
            !under("hash-after-the-fix").satisfied_by(&live),
            "a re-authorized account, or an edited block, needs its own process"
        );
        assert!(
            under("hash-after-the-fix").differences(&live).contains("credentials"),
            "and the report says which of the two it is: {}",
            under("hash-after-the-fix").differences(&live)
        );

        // The local login has no stored credential, and its fingerprint is
        // exactly what it always was.
        let local = Fingerprint::default();
        assert_eq!(local.credential, None);
        assert!(local.satisfied_by(&local));
        assert!(!local.satisfied_by(&live), "still not the account's process");
    }

    /// The question capability is recorded and never compared, in either
    /// direction: a resident from before `--permission-prompt-tool` is not
    /// restarted for it, and gets it at its next replacement of its own.
    #[test]
    fn being_able_to_ask_is_never_a_reason_to_restart() {
        let older = Fingerprint {
            model: Some("opus".into()),
            uploads: true,
            questions: false,
            ..Default::default()
        };
        let wants = Fingerprint {
            questions: true,
            ..older.clone()
        };
        assert!(wants.satisfied_by(&older), "a message from this server fits a resident from the last one");
        assert!(older.satisfied_by(&wants), "and the other way round");
        assert!(
            serde_json::from_str::<Fingerprint>(r#"{"model":"opus","uploads":true}"#).is_ok_and(|read| !read.questions),
            "a record written before the field reads as a process without it"
        );
    }

    #[test]
    fn only_a_widened_uploads_directory_is_forgiven_a_mismatch() {
        let live = Fingerprint {
            model: Some("opus".into()),
            uploads: true,
            ..Default::default()
        };
        assert!(
            Fingerprint {
                model: Some("opus".into()),
                uploads: false,
                ..Default::default()
            }
            .satisfied_by(&live),
            "a message with no attachment fits a process that can read the uploads directory"
        );
        assert!(live.satisfied_by(&live), "and one that needs the directory fits the process that has it");
        assert!(
            !Fingerprint {
                model: Some("opus".into()),
                uploads: true,
                ..Default::default()
            }
            .satisfied_by(&Fingerprint {
                model: Some("opus".into()),
                uploads: false,
                ..Default::default()
            }),
            "the other direction is a real mismatch: the flag is not there to be added later"
        );
        for different in [
            Fingerprint {
                model: Some("sonnet".into()),
                uploads: true,
                ..Default::default()
            },
            Fingerprint {
                model: Some("opus".into()),
                effort: Some("high".into()),
                uploads: true,
                ..Default::default()
            },
            Fingerprint {
                model: Some("opus".into()),
                permission: Some("acceptEdits".into()),
                uploads: true,
                ..Default::default()
            },
            Fingerprint {
                model: Some("opus".into()),
                account: Some("other".into()),
                uploads: true,
                ..Default::default()
            },
            Fingerprint {
                model: Some("opus".into()),
                chat: true,
                uploads: true,
                ..Default::default()
            },
        ] {
            assert!(
                !different.satisfied_by(&live),
                "every command-line setting needs its own process: {different:?}"
            );
            assert!(
                different.accepted_by(None),
                "…but a process nobody described is not a process to refuse a message: {different:?}"
            );
        }
    }

    /// The model the command line asked for and the model the CLI answers with
    /// are the same process, and a send may name either.
    ///
    /// The failure this pins is not theoretical: a resident spawned `--model
    /// fable` reports itself as `claude-fable-5`, the composer went on sending
    /// `fable`, and each send was read as a settings change — eight turns and
    /// eight restarts in one recording, each one killing the session's
    /// background work and failing whatever was queued behind it.
    #[test]
    fn the_model_a_spawn_asked_for_and_the_one_the_cli_answers_with_are_one_process() {
        let live = Fingerprint {
            model: Some("fable".into()),
            resolved: Some("claude-fable-5".into()),
            ..Default::default()
        };
        for name in ["fable", "claude-fable-5"] {
            assert!(
                Fingerprint {
                    model: Some(name.into()),
                    ..Default::default()
                }
                .satisfied_by(&live),
                "{name} names the process that is running"
            );
        }
        assert!(
            !Fingerprint {
                model: Some("sonnet".into()),
                ..Default::default()
            }
            .satisfied_by(&live),
            "and a third name is still a different model"
        );
        // The other side of it: what a message asks for is a name, never a
        // resolution, so a resolved name it happens to carry is not compared.
        assert!(
            Fingerprint {
                model: Some("fable".into()),
                resolved: Some("something else entirely".into()),
                ..Default::default()
            }
            .satisfied_by(&live),
            "the asking side's resolved name means nothing"
        );
    }

    /// A message that cannot be delivered says which setting is in the way.
    ///
    /// The only thing a re-delivered message can be told — its environment was
    /// never journalled, so there is no process to start for it — and "the
    /// settings were lost" told the person who sent it nothing they could act
    /// on. Every field the comparison refuses on has a name here.
    #[test]
    fn a_message_that_cannot_be_delivered_names_what_differs() {
        let live = Fingerprint {
            model: Some("haiku".into()),
            resolved: Some("claude-haiku-4-5".into()),
            account: Some("work".into()),
            ..Default::default()
        };
        let differences = |wanted: Fingerprint| wanted.differences(&live);

        assert_eq!(
            differences(Fingerprint {
                model: Some("opus".into()),
                account: Some("work".into()),
                ..Default::default()
            }),
            "model opus rather than haiku (answering as claude-haiku-4-5)",
            "a process with two names is reported under both, or the answer looks like the question"
        );
        assert_eq!(
            differences(Fingerprint {
                model: Some("opus".into()),
                resolved: None,
                effort: Some("high".into()),
                permission: None,
                account: None,
                credential: None,
                chat: true,
                uploads: true,
                questions: true,
            }),
            "model opus rather than haiku (answering as claude-haiku-4-5), effort high rather than none, \
             the local login rather than the work account, the chat persona, access to the uploads directory",
            "and every setting that needs its own process is named, in one line"
        );
    }

    /// What a restart makes of a backlog: who still has to be given what.
    ///
    /// The distinction the `steered` flag exists for. An attach re-attaches to
    /// the *same process*, and a message steered into it is in that process'
    /// stdin — surviving this server entirely, because stdin belongs to the
    /// process. Handing it back as an ordinary queued message would deliver it
    /// twice, which for a prompt means running its side effects twice.
    #[test]
    fn an_attach_leaves_a_steered_message_with_the_process_that_already_has_it() {
        let live = Fingerprint::default();
        let queued = |id: &str, text: &str, steered: bool| runs::Queued {
            id: id.to_string(),
            text: text.to_string(),
            fingerprint: live.clone(),
            steered,
            rotation_attempt: 0,
            continuation: None,
        };

        let inherited = inherited_queue(SESSION, &[queued("q-1", "urgent", true), queued("", "waiting", false)], Some(&live));
        assert_eq!(inherited.delivered_ahead.len(), 1, "the steered one is not re-delivered");
        assert_eq!(
            inherited.delivered_ahead[0].id, "q-1",
            "and it keeps its id, so a handle a client is holding still names it"
        );
        assert_eq!(inherited.queue.len(), 1);
        assert_eq!(inherited.queue[0].text, "waiting");
        assert!(
            !inherited.queue[0].id.is_empty(),
            "a record from before the queue was addressable gets an id rather than being dropped"
        );

        // The prefix rule is unchanged, and applies across both: the queue is
        // answered in order, so a message the running process cannot take takes
        // everything behind it with it.
        let mismatch = runs::Queued {
            id: "q-2".to_string(),
            text: "under another model".to_string(),
            fingerprint: Fingerprint {
                model: Some("opus".into()),
                ..Default::default()
            },
            steered: false,
            rotation_attempt: 0,
            continuation: None,
        };
        let inherited = inherited_queue(
            SESSION,
            &[queued("q-1", "urgent", true), mismatch.clone(), queued("q-3", "later", false)],
            Some(&live),
        );
        assert_eq!(inherited.delivered_ahead.len(), 1);
        assert!(inherited.queue.is_empty(), "nothing behind the mismatch survives");

        // …and it applies to a fingerprint, not to the absence of one. A record
        // written before the field existed says *unknown*, and unknown is not a
        // process description to judge messages against: the server that
        // accepted them knew what it was accepting, and this one knows less.
        let inherited = inherited_queue(SESSION, &[queued("q-1", "urgent", true), mismatch, queued("q-3", "later", false)], None);
        assert_eq!(inherited.delivered_ahead.len(), 1);
        assert_eq!(
            inherited.queue.len(),
            2,
            "an unrecorded fingerprint vetoes nothing: the whole backlog survives the restart"
        );
    }

    /// Nothing the CLI can write is allowed to be an error, and only four kinds
    /// of line are allowed to mean anything.
    #[test]
    fn an_unrecognisable_line_is_never_an_error() {
        assert_eq!(classify("not json at all"), Event::Unparseable);
        assert_eq!(classify(""), Event::Unparseable);
        assert_eq!(classify("{"), Event::Unparseable);
        // Valid JSON that is not an object, and objects with no type: neither is
        // a line to act on, and neither is a failure.
        assert_eq!(classify("[1,2,3]"), Event::Other);
        assert_eq!(classify("{\"no\":\"type\"}"), Event::Other);
        // Types this driver does not act on, including ones invented later.
        for line in [
            r#"{"type":"stream_event"}"#,
            r#"{"type":"rate_limit_event"}"#,
            r#"{"type":"a_type_that_does_not_exist_yet"}"#,
        ] {
            assert_eq!(classify(line), Event::Other, "{line}");
        }
        // A `system` line this driver does not act on is still known for what it
        // is, subtypes invented later included, and a task event too malformed
        // to read is one of those rather than an unknown line. The distinction
        // is the replay's: an unknown line may be a turn's output and a `system`
        // line never is.
        for line in [
            r#"{"type":"system","subtype":"task_started"}"#,
            r#"{"type":"system","subtype":"commands_changed","commands":[]}"#,
            r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":50}"#,
            r#"{"type":"system","subtype":"something_new_in_2_2"}"#,
        ] {
            assert_eq!(classify(line), Event::Housekeeping, "{line}");
        }
        assert_eq!(classify(r#"{"type":"control_response"}"#), Event::ControlResponse);
    }

    /// claude asking *us* something: the two shapes 2.1.295 writes once it has
    /// `--permission-prompt-tool stdio`, and the withdrawal an interrupt makes.
    #[test]
    fn a_permission_request_and_its_withdrawal_are_read_for_what_they_are() {
        let question = classify(
            r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","display_name":"AskUserQuestion","input":{"questions":[{"question":"Which colour do you prefer?","header":"Colour","options":[{"label":"Red","description":"Warm"},{"label":"Blue","description":"Cool"}],"multiSelect":false}]},"tool_use_id":"toolu_1","requires_user_interaction":true}}"#,
        );
        let Event::Permission(request) = question else {
            panic!("a permission request: {question:?}")
        };
        assert!(request.is_question());
        assert_eq!((request.request_id.as_str(), request.tool_use_id.as_deref()), ("req-1", Some("toolu_1")));

        let write = classify(
            r#"{"type":"control_request","request_id":"req-2","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"file_path":"/etc/hosts"},"tool_use_id":"toolu_2"}}"#,
        );
        assert!(matches!(&write, Event::Permission(request) if !request.is_question()), "{write:?}");

        assert_eq!(
            classify(r#"{"type":"control_cancel_request","request_id":"req-1"}"#),
            Event::ControlCancel { request_id: "req-1".into() }
        );
        assert_eq!(
            classify(r#"{"type":"control_request","request_id":"req-3","request":{"subtype":"hook_callback"}}"#),
            Event::ControlRequest {
                request_id: Some("req-3".into())
            },
            "a request this server does not handle is still answered, so it is told apart"
        );
        assert_eq!(classify(r#"{"type":"control_cancel_request"}"#), Event::Other);
    }

    /// The one field that tells this session's own thread from a subagent's.
    ///
    /// The whole of [`Driver::on_speech`] rests on it, so it is asserted against
    /// the shapes a real session writes rather than against a schema: a
    /// forwarded child event carries the `parent_tool_use_id` of the `Task` that
    /// started it, and the main thread's own events carry `null` or nothing.
    #[test]
    fn a_forwarded_subagent_line_is_not_the_main_thread_talking() {
        for line in [
            r#"{"type":"assistant","message":{"model":"claude-fable-5"}}"#,
            r#"{"type":"assistant","parent_tool_use_id":null,"message":{}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result"}]}}"#,
            r#"{"type":"user","parent_tool_use_id":null,"message":{}}"#,
        ] {
            assert_eq!(classify(line), Event::Speech, "{line}");
        }
        for line in [
            r#"{"type":"assistant","parent_tool_use_id":"toolu_01HuJ","message":{"model":"claude-opus-5"}}"#,
            r#"{"type":"user","parent_tool_use_id":"toolu_01HuJ","message":{}}"#,
        ] {
            assert_eq!(classify(line), Event::Other, "{line}");
        }
        assert_eq!(
            classify(r#"{"type":"system","subtype":"task_updated","task_id":"t1"}"#),
            Event::Housekeeping
        );
        // A `system` line is never the main thread working, whatever its parent
        // field says: these go on while the session sits idle. `task_progress`
        // is read for what it says about the task and is still not speech.
        assert!(matches!(
            classify(r#"{"type":"system","subtype":"task_progress","task_id":"t1","parent_tool_use_id":null}"#),
            Event::TaskProgress { .. }
        ));
    }

    /// Everything a background panel can show about a running task, which the
    /// stream reports every few seconds and this server used to drop.
    #[test]
    fn a_progress_line_fills_in_the_task_it_names() {
        let line = r#"{"type":"system","subtype":"task_progress","task_id":"t1","tool_use_id":"toolu_09",
            "description":"Reading src/app.rs","subagent_type":"general-purpose",
            "usage":{"total_tokens":10205,"tool_uses":1,"duration_ms":3169},"last_tool_name":"Bash"}"#;
        let Event::TaskProgress { progress } = classify(line) else {
            panic!("a task_progress line is its own event");
        };

        let mut tasks = BackgroundTasks::default();
        assert!(!tasks.progress(progress.clone()), "a task nobody announced is not invented here");
        tasks.observe(TaskSeen {
            id: "t1".into(),
            tool_use_id: None,
            description: Some("Sweep the parser".into()),
            task_type: Some("local_agent".into()),
        });
        assert!(tasks.progress(progress.clone()), "the first progress line is news");
        assert!(!tasks.progress(progress), "the same numbers again are not");

        let task = &tasks.wire()[0];
        assert_eq!(task.last_tool_name.as_deref(), Some("Bash"));
        assert_eq!(task.subagent_type.as_deref(), Some("general-purpose"));
        assert_eq!(
            (task.total_tokens.0, task.tool_uses.0, task.duration_ms.0),
            (Some(10205), Some(1), Some(3169))
        );
        assert_eq!(
            task.tool_use_id.0.as_deref(),
            Some("toolu_09"),
            "progress carries the join key a restart's snapshot lacks"
        );
        assert_eq!(
            task.description.0.as_deref(),
            Some("Sweep the parser"),
            "progress describes the tool of the moment, never what the task was asked for"
        );
    }

    #[test]
    fn the_two_lines_that_matter_are_read_with_defaults_rather_than_a_schema() {
        assert_eq!(
            classify(r#"{"type":"system","subtype":"init","session_id":"abc","model":"claude-haiku-4-5-20251001","tools":[]}"#),
            Event::Init {
                session: Some("abc".into()),
                model: Some("claude-haiku-4-5-20251001".into())
            }
        );
        assert_eq!(
            classify(r#"{"type":"system","subtype":"init"}"#),
            Event::Init { session: None, model: None },
            "an init with no id still means the process is listening"
        );
        assert_eq!(
            classify(r#"{"type":"result","subtype":"success","is_error":false,"result":"hi"}"#),
            Event::Result {
                is_error: false,
                subtype: "success".into(),
                detail: Some("hi".into()),
                api_status: None,
            }
        );
        assert_eq!(
            classify(r#"{"type":"result"}"#),
            Event::Result {
                is_error: false,
                subtype: "an error".into(),
                detail: None,
                api_status: None,
            },
            "a result stripped of every field is still the end of the turn"
        );
    }

    /// The limit log's two claude inputs, as claude 2.1.266 writes them.
    ///
    /// A session limit is a `result` with `is_error: true` and — confusingly —
    /// `subtype: success`, carrying the HTTP status beside the words. And the
    /// `rate_limit_event` before it says `rejected`, where the one before
    /// nearly every other turn says `allowed`: only the refusal is an event.
    #[test]
    fn a_limit_result_and_a_refused_rate_limit_event_are_read_for_the_log() {
        assert_eq!(
            classify(
                r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":429,"terminal_reason":"api_error","result":"You've hit your session limit · resets 2:30am (Asia/Tokyo)"}"#
            ),
            Event::Result {
                is_error: true,
                subtype: "success".into(),
                detail: Some("You've hit your session limit · resets 2:30am (Asia/Tokyo)".into()),
                api_status: Some(429),
            }
        );

        let rejected = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1789534200,"rateLimitType":"five_hour"},"uuid":"u","session_id":"s"}"#;
        match classify(rejected) {
            Event::RateLimit { info } => {
                assert_eq!(info["rateLimitType"], "five_hour");
                assert_eq!(info["resetsAt"], 1789534200);
            }
            other => panic!("a refusal is its own event, got {other:?}"),
        }
        for status in ["allowed", "allowed_warning"] {
            let line = format!(
                r#"{{"type":"rate_limit_event","rate_limit_info":{{"status":"{status}","resetsAt":1790157600,"rateLimitType":"five_hour"}}}}"#
            );
            assert_eq!(classify(&line), Event::Other, "a reading is not an event: {status}");
        }
    }

    /// Verbatim: the `allowed` event claude writes before nearly every turn.
    const ALLOWED_WITH_WINDOWS: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"isUsingOverage":false,"overageDisabledReason":"org_level_disabled","overageStatus":"rejected","rateLimitType":"five_hour","resetsAt":1790436000,"status":"allowed","unifiedWindows":{"five_hour":{"resetsAt":1790436000,"utilization":0.19},"seven_day":{"resetsAt":1790460000,"utilization":0.46}}}}"#;

    /// An `allowed` event is not a refusal, but its windows are a reading, so
    /// it is told apart from the lines that say nothing — and a refusal carries
    /// its windows through to the same place.
    #[test]
    fn a_rate_limit_event_with_windows_is_a_usage_reading_whatever_its_status() {
        match classify(ALLOWED_WITH_WINDOWS) {
            Event::UsageReading { info } => {
                assert_eq!(info["status"], "allowed");
                let windows = crate::quota::readings::windows_from_rate_limit_info(&info);
                assert_eq!(windows.len(), 2);
                assert_eq!(windows[0].label, "5h");
                assert_eq!(windows[0].used_percent, 19.0);
            }
            other => panic!("an allowed event with windows is a reading, got {other:?}"),
        }
        let rejected = r#"{"type":"rate_limit_event","rate_limit_info":{"isUsingOverage":false,"overageDisabledReason":"org_level_disabled","overageStatus":"rejected","rateLimitType":"seven_day_overage_included","resetsAt":1790575200,"status":"rejected","unifiedWindows":{"five_hour":{"resetsAt":1790436000,"utilization":0.02},"seven_day":{"resetsAt":1790575200,"utilization":0.82},"seven_day_overage_included":{"resetsAt":1790575200,"utilization":1}}}}"#;
        match classify(rejected) {
            Event::RateLimit { info } => {
                let labels: Vec<String> = crate::quota::readings::windows_from_rate_limit_info(&info)
                    .into_iter()
                    .map(|w| w.label)
                    .collect();
                assert_eq!(
                    labels,
                    ["5h", "weekly", "weekly (Fable)"],
                    "the refusal is still recorded as one, windows and all"
                );
            }
            other => panic!("a refusal stays a refusal, got {other:?}"),
        }
        // No windows, nothing to file.
        assert_eq!(
            classify(r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","unifiedWindows":null}}"#),
            Event::Other
        );
    }

    /// Which login a session's reading is filed under, from the environment the
    /// server really builds for each kind of spawn.
    #[test]
    fn a_usage_reading_goes_to_the_login_the_process_runs_on() {
        use crate::claude_roots::ClaudeRoot;
        use crate::quota::readings::session_login;
        use crate::quota::UsageLogin;

        let local = crate::process_env::claude_env_bare();
        let managed = crate::process_env::claude_env([("CLAUDE_CONFIG_DIR", "/data/instances/default/claude")]);
        assert_eq!(recipe_root(&local), ClaudeRoot::Local, "a local-login spawn names no config dir");
        assert_eq!(recipe_root(&managed), ClaudeRoot::Managed);

        let id = "0b6f7a7e-2f7a-4d8e-9d0c-1a2b3c4d5e6f";
        // An account session: the account, in whichever root.
        assert_eq!(session_login(Some(id), None), Some(UsageLogin::Account(id.into())));
        // No account, the user's own `~/.claude`: the local login.
        assert_eq!(session_login(None, Some(recipe_root(&local))), Some(UsageLogin::Local));
        // No account, the managed root: some other login — skipped.
        assert_eq!(session_login(None, Some(recipe_root(&managed))), None);
    }

    #[test]
    fn a_failure_tail_keeps_the_last_lines_and_nothing_longer() {
        let stderr = (1..=10).map(|n| format!("line {n}\n")).collect::<String>();
        let tail = last_lines(&stderr, 6);
        assert_eq!(tail.lines().count(), 6);
        assert!(tail.starts_with("line 5"));
        assert!(!tail.contains("line 4"));
        assert_eq!(last_lines("only one", 6), "only one");
        assert_eq!(last_lines("", 6), "");
        assert_eq!(clip("short"), "short");
        assert_eq!(clip(&"x".repeat(LOG_LINE_CAP + 10)).chars().count(), LOG_LINE_CAP + 1);
    }

    // --------------------------------------------------------- the harness

    /// A driver, the fake claude it is driving, and the files both write.
    struct Harness {
        _home: tempfile::TempDir,
        registry: Arc<InjectionRegistry>,
        tx: mpsc::UnboundedSender<Cmd>,
        shared: Arc<Mutex<ResidentState>>,
        /// Every line the fake claude read off its stdin, in order.
        log: PathBuf,
        /// Touched to let the fake claude finish the turn it is holding.
        go: PathBuf,
        /// Where the `claude` a respawn will find lives.
        bin: PathBuf,
        /// One line of argv per process the driver has started for itself.
        spawns: PathBuf,
        /// Every line a *replacement* process read off its stdin.
        after: PathBuf,
        shim: tokio::task::JoinHandle<Result<(), String>>,
        driver: tokio::task::JoinHandle<()>,
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            self.driver.abort();
            self.shim.abort();
            // A respawn starts a real shim and a real process, and neither is a
            // child of anything this test owns — that is the whole point of the
            // shim. Aborting the task that was talking to one leaves it running
            // against a `tempdir` that is about to go, so the journal is read
            // for what to end. claude leads a process group of its own, and the
            // shim follows its child out.
            for record in self.registry.journal.read() {
                // SAFETY: `kill` delivers a signal to a process group and cannot
                // corrupt this process' own state.
                unsafe { libc::kill(-(record.cli_pid as libc::pid_t), libc::SIGKILL) };
            }
        }
    }

    /// A registry with timings a test can wait out.
    ///
    /// Built field by field rather than through [`InjectionRegistry::new`] on
    /// purpose: `new` reads the timings out of the environment, which is
    /// process-global and shared with every other test in this binary.
    fn registry(home: &Path, stall: Duration, tick: Duration, step: Duration, settle: Duration, absorb_poll: Duration) -> Arc<InjectionRegistry> {
        let paths = Arc::new(Paths::new(home.to_path_buf()));
        let dirs = InstanceDirs::new(&paths.data_dir(), crate::instance::DEFAULT_INSTANCE).expect("valid name");
        dirs.create().expect("instance directories");
        Arc::new(InjectionRegistry {
            receipts: Arc::new(crate::inject::receipts::Receipts::new(crate::db::at(paths.instance_db()))),
            questions: Arc::new(crate::inject::questions::Questions::new(crate::db::at(paths.instance_db()))),
            accounts: Arc::new(crate::accounts::store::AccountsStore::new(paths.clone())),
            quota: Arc::new(crate::quota::QuotaCache::new(paths.clone())),
            server: crate::sessions::ServerId::local(&paths),
            codex_servers: Mutex::new(HashMap::new()),
            runs: Mutex::new(HashMap::new()),
            residents: Mutex::new(HashMap::new()),
            spawn_gate: tokio::sync::Mutex::new(()),
            shim_dir: dirs.session_shims(),
            idle_timeout: Duration::from_secs(600),
            stall_after: stall,
            interrupt_step: step,
            watchdog_interval: tick,
            resident_idle: None,
            // Short enough that the ceiling on a parked settings change can be
            // watched inside a test, and far past anything else here waits for.
            await_background: Some(Duration::from_millis(400)),
            // The production default, set here rather than read for the reason
            // the timings are: no test's answer may depend on the shell.
            rotate_ahead: Some(crate::accounts::rotation::AHEAD_DEFAULT_PERCENT),
            // This test's own, so the corroboration probe reads a directory the
            // test controls rather than whatever `/tmp` happens to hold.
            task_root: home.join("task-root"),
            recovery_backoff: [Duration::from_millis(30); 3],
            steer_settle: settle,
            absorb_poll,
            journal: runs::RunsFile::new(dirs.runs_file()),
            lock_dir: paths.data_dir().join("inject-locks"),
            // Recording on, into this test's own instance directory, for the
            // same reason the timings are set by hand: `Recorders::new` reads
            // the environment, and `a_session_records_its_whole_life` needs the
            // answer to be yes whatever the developer's shell says.
            recorders: crate::record::Recorders::new_at(dirs.session_logs(), None, paths.clone()),
            instance: dirs,
            listeners: std::sync::OnceLock::new(),
            reading_recheck: Mutex::new(None),
            drafts: crate::inject::drafts::Drafts::default(),
            paths,
        })
    }

    /// The bounded wait every fake CLI in here uses to sit on `$GO`, prepended
    /// to every script [`Harness::build`] writes.
    ///
    /// A stub that outlives its test is not a slow test, it is a fork bomb. The
    /// harness' `tempdir` goes with the panic, `$GO` can then never appear, and
    /// a reparented `sh` polls a path that no longer exists at fifty spawns a
    /// second for as long as the machine stays up. Enough of them and the box
    /// stops resolving user names. So the wait is bounded twice over:
    ///
    /// * it gives up the instant the directory holding `$GO` is gone, which is
    ///   the leak exactly — the release file cannot arrive from a `tempdir`
    ///   that has been removed, so there is nothing left to wait for; and
    /// * it gives up after thirty seconds regardless, which covers the case
    ///   [`Drop`] cannot — a test binary killed outright never unwinds, so the
    ///   directory survives and the anchor above never fires.
    ///
    /// Thirty seconds rather than the eight the stepped `await` helpers use
    /// because `$GO` gates a whole turn while those only wait on a nudge a test
    /// is about to make: [`until`] gives up at ten seconds, and a cap near it
    /// would race the assertion for which failure gets reported. Giving up
    /// *exits* rather than falling through, and with a code no test expects, so
    /// a genuine hang still fails loudly instead of passing quietly.
    const WAIT_GUARD: &str = r#"
GO_DIR=$(dirname "$GO")
await_go() {
  n=0
  while [ ! -f "$GO" ]; do
    [ -d "$GO_DIR" ] || exit 97
    [ "$n" -lt 1500 ] || exit 97
    sleep 0.02
    n=$((n + 1))
  done
}
"#;

    /// The lines a real claude writes before it will take anything.
    ///
    /// Two of the three are the point. `hook_started` comes *before* `init`, so
    /// a driver that took the first line as the handshake would already be
    /// wrong; and the junk line proves the tolerant parse, since a reader that
    /// refused it would never reach the init behind it.
    fn preamble() -> String {
        [
            r#"printf '%s\n' '{"type":"system","subtype":"hook_started"}'"#.to_string(),
            r#"printf '%s\n' 'this line is not json'"#.to_string(),
            format!(r#"printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'"#),
        ]
        .join("\n")
    }

    /// The ordinary body: log every message, hold the turn until released.
    fn hold_until_released() -> &'static str {
        r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
    *control_request*) : ;;
    *) await_go
       rm -f "$GO"
       printf '%s\n' '{"type":"assistant","message":{}}'
       printf '%s\n' '{"type":"result","subtype":"success","is_error":false}' ;;
  esac
done
"#
    }

    /// The CLI that **folds** a mid-turn message into the turn it interrupted.
    ///
    /// One of the two things claude 2.1.231 really does with a `user` line
    /// written while a turn is running, and the one that used to wedge a
    /// session: the message is injected at the next tool boundary, the model
    /// answers both prompts in one reply, and the *original* turn's `result` is
    /// the only one that ever goes out. No `init`, no second `result`, no echo
    /// of the injected text — nothing on stdout distinguishes it at all.
    ///
    /// Modelled by reading the steered line before the turn is released rather
    /// than by peeking at the pipe, which no portable `sh` can do: the second
    /// `read` blocks until the steer arrives, and the one `result` after it
    /// covers both messages. Everything after that turn is ordinary.
    fn folds_the_steered_message() -> &'static str {
        r#"
IFS= read -r first
printf '%s\n' "$first" >> "$LOG"
IFS= read -r second
printf '%s\n' "$second" >> "$LOG"
await_go
rm -f "$GO"
printf '%s\n' '{"type":"assistant","message":{}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
    *control_request*) : ;;
    *) await_go
       rm -f "$GO"
       printf '%s\n' '{"type":"result","subtype":"success","is_error":false}' ;;
  esac
done
"#
    }

    /// The CLI that **buffers** a mid-turn message and runs it as its own turn.
    ///
    /// The other half of the same CLI, and the half [`preamble`] cannot show:
    /// every turn after the first announces itself with a `system:init` of its
    /// own before it does anything, which is precisely the evidence
    /// [`Driver::settle_delivered`] waits for. The turn is then held, so a
    /// settle window that fired anyway would be caught: the message spends far
    /// longer than the window running.
    fn defers_the_steered_message() -> String {
        format!(
            r#"
first=1
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
    *control_request*) continue ;;
  esac
  if [ "$first" = 0 ]; then
    printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'
  fi
  first=0
  await_go
  rm -f "$GO"
  printf '%s\n' '{{"type":"assistant","message":{{}}}}'
  printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}'
done
"#
        )
    }

    /// The CLI that runs a turn of its own when a background task finishes.
    ///
    /// The production sequence, one step per file the test touches: a subagent's
    /// stream is forwarded while the session sits between turns, then the
    /// notification wakes the main agent and it runs a whole turn — `init`, an
    /// assistant message of its own, `result` — with nothing on stdin to have
    /// started it. See [`Driver::on_speech`].
    ///
    /// Written as a subshell beside the ordinary read loop because that is
    /// exactly what the real thing is: output arriving when nothing has been
    /// asked for. The two never print at the same time — the loop only answers a
    /// prompt, and no prompt can be delivered until the woken turn's `result`
    /// has gone out — so the interleaving one shared pipe would allow cannot
    /// happen here.
    ///
    /// Each step ends with a `background_tasks_changed` the driver visibly acts
    /// on, so a test can wait for "everything up to here has been read" instead
    /// of sleeping and hoping. Every wait is bounded: a test that fails halfway
    /// leaves no subshell polling a `tempdir` that has gone.
    fn wakes_itself_for_a_finished_task() -> String {
        format!(
            r#"
dir=$(dirname "$GO")
await() {{ n=0; while [ ! -f "$dir/$1" ] && [ "$n" -lt 400 ]; do [ -d "$dir" ] || exit 97; sleep 0.02; n=$((n + 1)); done; }}
(
  await subagent
  printf '%s\n' '{{"type":"system","subtype":"task_progress","task_id":"t1"}}'
  printf '%s\n' '{{"type":"assistant","parent_tool_use_id":"toolu_child","message":{{"model":"claude-opus-5"}}}}'
  printf '%s\n' '{{"type":"user","parent_tool_use_id":"toolu_child","message":{{}}}}'
  printf '%s\n' '{{"type":"system","subtype":"background_tasks_changed","tasks":[{{"task_id":"t1"}}]}}'
  await wake
  printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'
  printf '%s\n' '{{"type":"system","subtype":"background_tasks_changed","tasks":[]}}'
  await speak
  printf '%s\n' '{{"type":"assistant","message":{{"model":"claude-fable-5"}}}}'
  await finish
  printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}'
) &
{}
"#,
            hold_until_released()
        )
    }

    /// Put a `claude` in `bin` that a real spawn will find and run.
    fn write_claude(bin: &Path, body: &str) {
        let path = bin.join("claude");
        std::fs::write(&path, body).expect("fake claude");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }

    /// A recipe whose environment can actually start the `claude` in `bin`.
    fn recipe(bin: &Path, spawns: &Path, after: &Path, fingerprint: Fingerprint) -> Recipe {
        let mut env = Env::new();
        env.insert("PATH".into(), format!("{}:/bin:/usr/bin", bin.display()));
        env.insert("SPAWNS".into(), spawns.to_string_lossy().into_owned());
        env.insert("AFTER".into(), after.to_string_lossy().into_owned());
        env.insert("REFUSE".into(), bin.join("refuse").to_string_lossy().into_owned());
        Recipe {
            args: vec![],
            env,
            workdir: Some(bin.to_path_buf()),
            fingerprint,
        }
    }

    /// The `claude` a respawn finds on the recipe's `PATH`, by default.
    ///
    /// The same handshake as [`preamble`] and the same one-result-per-line body,
    /// plus the two things a recovery test has to be able to see: which argv the
    /// driver chose — `--resume`, or `--resume --fork-session` — and which id the
    /// replacement answered under. A `--fork-session` announces a *different* id,
    /// because that is exactly what the real CLI does and what makes the second
    /// rung worth having.
    fn replacement_claude(session: &str, forked: &str) -> String {
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$SPAWNS"
[ -f "$REFUSE" ] && exit 91
answer='{session}'
case "$*" in *--fork-session*) answer='{forked}' ;; esac
printf '%s\n' '{{"type":"system","subtype":"hook_started"}}'
printf '{{"type":"system","subtype":"init","session_id":"%s"}}\n' "$answer"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$AFTER"
  case "$line" in
    *control_request*) : ;;
    *) printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}' ;;
  esac
done
"#
        )
    }

    /// What a harness' driver is told it inherited.
    ///
    /// One argument rather than three because [`Harness::build`] already takes
    /// as many as anything should, and because the three only ever vary
    /// together: they are the whole of what [`InjectionRegistry::attached`]
    /// reconstructs from a journal record.
    /// What a replayed background task has left on disk to be believed by.
    ///
    /// The distinction the last three draw is the whole point of reading a
    /// directory's *entries*: claude creates a transcript when a background
    /// agent starts and then streams into it for as long as the agent runs, so
    /// a directory whose mtime is minutes old routinely holds a file written a
    /// second ago. A probe that reads only the directory calls that quiet.
    #[derive(Clone, Copy)]
    enum Witness {
        /// Nothing was written anywhere: the task is a ghost.
        None,
        /// A transcript written now, in a directory created now — a task that
        /// started moments before the attach.
        Fresh,
        /// A transcript written now, in a directory last touched long ago — a
        /// task that started minutes ago and is still streaming. The live case
        /// this harness exists to pin down.
        StreamingIntoAnOldDirectory,
        /// A transcript and a directory both long untouched: the agent is gone.
        Stale,
        /// No transcript at all, and a task output file written now — a
        /// background *command*, which writes nothing under `subagents/`.
        TaskOutputOnly,
    }

    impl Witness {
        /// The shell that leaves this witness, run by the fake claude before it
        /// writes its history.
        ///
        /// `touch -t` backdates rather than sleeps: the quiet window is thirty
        /// seconds and no test may wait that long. Backdating the directory
        /// *after* the file is created is what makes the two differ at all —
        /// creating an entry is precisely what moves a directory's mtime, so the
        /// order is the test.
        fn script(self, subagents: &Path, tasks: &Path) -> String {
            const LONG_AGO: &str = "202001010000";
            let (dir, file) = (subagents.display(), "agent-ghost.jsonl");
            match self {
                Witness::None => String::new(),
                Witness::Fresh => format!("mkdir -p {dir} && printf '{{}}\\n' > {dir}/{file}\n"),
                Witness::StreamingIntoAnOldDirectory => format!("mkdir -p {dir} && printf '{{}}\\n' > {dir}/{file} && touch -t {LONG_AGO} {dir}\n"),
                Witness::Stale => format!(
                    "mkdir -p {dir} && printf '{{}}\\n' > {dir}/{file} \
                     && touch -t {LONG_AGO} {dir}/{file} && touch -t {LONG_AGO} {dir}\n"
                ),
                Witness::TaskOutputOnly => {
                    format!("mkdir -p {0} && printf 'working\\n' > {0}/ghost.output\n", tasks.display())
                }
            }
        }
    }

    struct Adoption {
        /// `Some(witness)` for a driver that finds history waiting for it,
        /// `None` for a fresh spawn; see [`Harness::start_attached`].
        attach: Option<Witness>,
        /// What the record says the running process was started with. `None` is
        /// a record from before the journal held a fingerprint.
        live: Option<Fingerprint>,
        /// The backlog the record was holding, as the journal kept it: no
        /// recipe, because a journal cannot carry an environment.
        queued: Vec<runs::Queued>,
        /// The fingerprint of the recipe the driver holds, when it is not the
        /// default: a process started under an account, for the rotation tests.
        recipe: Option<Fingerprint>,
        /// A queue the record says was held after a limit, as a server that
        /// restarted mid-hold finds it.
        hold: Option<Hold>,
        /// The shim's replay ring, when a test needs one small enough to lose
        /// the start of the history; `None` is the production size.
        ring: Option<usize>,
        /// What the session's transcript says when the driver attaches, written
        /// and backdated past [`TRANSCRIPT_SETTLES_AFTER`] — so the transcript is
        /// a witness the replay's settle really asks. Attach only.
        transcript: Option<String>,
        /// How often the driver reads the transcript for folded steers. Short
        /// by default, so a test watching for one sees it inside its own wait;
        /// [`NEVER_SETTLES`] for a test that has to show the feed's nudge alone
        /// is enough.
        absorb_poll: Duration,
    }

    impl Default for Adoption {
        /// A fresh spawn, whose settings are known because this driver chose
        /// them. Hand-written rather than derived precisely because the derived
        /// `None` would mean "unknown", which is a different test.
        fn default() -> Self {
            Adoption {
                attach: None,
                live: Some(Fingerprint::default()),
                queued: Vec::new(),
                recipe: None,
                hold: None,
                ring: None,
                transcript: None,
                absorb_poll: Duration::from_millis(50),
            }
        }
    }

    /// A settle window no test can wait out.
    ///
    /// The default for every harness but the two steering scenarios, and the
    /// right default: with it, a delivered turn is ended by claude's own
    /// `result` and by nothing else, which is what every other test in here is
    /// about. The two that *are* about the window ask for a millisecond one.
    const NEVER_SETTLES: Duration = Duration::from_secs(3600);

    impl Harness {
        /// Run `body` as the fake claude, with a driver attached to it.
        async fn start(body: &str, stall: Duration, tick: Duration, step: Duration) -> Harness {
            Harness::start_with_handshake(&preamble(), body, stall, tick, step).await
        }

        /// The ordinary timings, but with a settle window a test can watch.
        ///
        /// The steering scenarios and nothing else: what they are testing *is*
        /// the window, so it has to be short enough to sit through and long
        /// enough that a fake claude which answers immediately still wins the
        /// race it is supposed to win. `step` is the interrupt ladder's, taken
        /// here because the one thing the two timings must not do is arrive in
        /// the wrong order — production's are two seconds and five.
        async fn settling(body: &str, settle: Duration, step: Duration) -> Harness {
            Harness::build(
                &preamble(),
                body,
                Duration::from_secs(3600),
                Duration::from_secs(3600),
                step,
                settle,
                Adoption::default(),
            )
            .await
        }

        /// A driver that finds history waiting for it, as a re-attach does.
        ///
        /// The shim is started, the fake claude writes `history` and touches a
        /// marker, and only then does the driver connect — so `link.total()` is
        /// non-zero and the driver walks the history through
        /// [`Driver::replay`] exactly as [`InjectionRegistry::attached`] makes
        /// it. Waiting on the marker rather than on a sleep is what keeps the
        /// arrangement from being a race.
        ///
        /// `corroborate` writes a subagent transcript from inside the fake
        /// claude, which is the only place it can be written *before* the
        /// driver settles its replay — and where a real one would come from.
        async fn start_attached(history: &str, body: &str, stall: Duration, tick: Duration, step: Duration, witness: Witness) -> Harness {
            Harness::build(
                history,
                body,
                stall,
                tick,
                step,
                NEVER_SETTLES,
                Adoption {
                    attach: Some(witness),
                    ..Adoption::default()
                },
            )
            .await
        }

        /// A driver that took over a resident whose record says `live`, holding
        /// `queued`, exactly as [`InjectionRegistry::attached`] builds one.
        ///
        /// The production scenario in one call: `None` is the record written
        /// before the journal carried a fingerprint at all.
        async fn start_inherited(live: Option<Fingerprint>, queued: Vec<runs::Queued>) -> Harness {
            Harness::build(
                &preamble(),
                hold_until_released(),
                Duration::from_secs(3600),
                Duration::from_secs(3600),
                Duration::from_millis(120),
                NEVER_SETTLES,
                Adoption {
                    attach: None,
                    live,
                    queued,
                    recipe: None,
                    hold: None,
                    ..Adoption::default()
                },
            )
            .await
        }

        async fn start_with_handshake(handshake: &str, body: &str, stall: Duration, tick: Duration, step: Duration) -> Harness {
            Harness::build(handshake, body, stall, tick, step, NEVER_SETTLES, Adoption::default()).await
        }

        async fn build(
            handshake: &str,
            body: &str,
            stall: Duration,
            tick: Duration,
            step: Duration,
            settle: Duration,
            adoption: Adoption,
        ) -> Harness {
            let home = tempfile::tempdir().expect("tempdir");
            Harness::build_in(home, handshake, body, stall, tick, step, settle, adoption).await
        }

        /// [`Harness::build`] in a home the caller has already put things in —
        /// the accounts a rotation test needs before the driver is told which
        /// one it runs under.
        #[allow(clippy::too_many_arguments)]
        async fn build_in(
            home: tempfile::TempDir,
            handshake: &str,
            body: &str,
            stall: Duration,
            tick: Duration,
            step: Duration,
            settle: Duration,
            adoption: Adoption,
        ) -> Harness {
            let Adoption {
                attach,
                live,
                queued,
                recipe: recipe_fingerprint,
                hold,
                ring,
                transcript: tail,
                absorb_poll,
            } = adoption;
            let registry = registry(home.path(), stall, tick, step, settle, absorb_poll);
            let log = home.path().join("stdin.log");
            let go = home.path().join("go");
            let bin = home.path().join("bin");
            let spawns = home.path().join("spawns.txt");
            let after = home.path().join("after.log");
            std::fs::create_dir_all(&bin).expect("bin");
            write_claude(&bin, &replacement_claude(SESSION, FORKED));
            let marked = home.path().join("history-written");
            // The subagent transcript a corroborated background task leaves
            // behind, written where a real one is: under the session's own
            // transcript, by the process itself.
            let transcript = transcript_path(home.path());
            let subagents = crate::sessions::transcript::subagent_dir(&transcript);
            // Where claude would put this session's task output: under the root
            // the test registry owns, behind a slug the probe never has to
            // guess — it finds the session id one level down instead.
            let tasks = registry.task_root.join("a-project").join(SESSION).join("tasks");
            let prologue = match attach {
                None => String::new(),
                Some(witness) => {
                    // The session's own transcript exists either way, so the
                    // corroboration probe really runs: with no transcript at all
                    // it would answer "nothing running" without looking, and the
                    // test would prove nothing about the look.
                    let witness = witness.script(&subagents, &tasks);
                    let tail = match &tail {
                        Some(tail) => format!("printf '%s\\n' '{tail}' > {0} && touch -t 202001010000 {0}\n", transcript.display()),
                        None => String::new(),
                    };
                    format!("touch {}\n{witness}{tail}touch {}\n", transcript.display(), marked.display())
                }
            };
            // `WAIT_GUARD` goes in ahead of everything a body might do, so
            // every fake CLI — including any written later — has a bounded,
            // self-destructing `await_go` rather than a loop that can outlive
            // the `tempdir` two lines above it.
            let script = format!(
                "LOG={}\nGO={}\nTRANSCRIPT={}\n{WAIT_GUARD}{}\n{}{}",
                log.display(),
                go.display(),
                transcript.display(),
                handshake,
                prologue,
                body
            );

            let sidecars = Sidecars::new(registry.shim_dir.clone(), "resident-under-test");
            sidecars.prepare().expect("sidecars");
            let shim = tokio::spawn(shim::run(shim::Args {
                socket: sidecars.socket().to_path_buf(),
                stderr_file: sidecars.stderr(),
                exit_file: sidecars.exit(),
                buffer_bytes: ring.unwrap_or(shim::DEFAULT_BUFFER_BYTES),
                command: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), script],
            }));
            if attach.is_some() {
                until("the history to be written", || marked.exists()).await;
            }
            let link = shim::connect_in_process(sidecars.socket()).await.expect("the shim answers");
            // Exactly what `InjectionRegistry::attached` computes: a shim that
            // has buffered bytes has history, and history has to be replayed
            // before anything in it may be acted on. A fresh spawn never
            // replays, however fast its CLI got going.
            let replaying = attach.is_some() && link.total() > 0;
            assert_eq!(replaying, attach.is_some(), "an attach has history to replay and a spawn has none");

            // The journal record this driver will keep up to date. Every harness
            // has one — a driver whose record names nothing writes its progress
            // into the void, and two of the things worth asserting about a
            // restart are what it wrote down.
            registry.journal.add(runs::RunRecord {
                codex: None,
                codex_account: None,
                hold: hold.clone(),
                run_id: "record-under-test".to_string(),
                source: Source::Claude,
                session_id: SESSION.to_string(),
                cwd: None,
                cli_pid: link.claude_pid(),
                started_at: iso8601(std::time::SystemTime::now()),
                pid_token: procs::start_token(link.claude_pid()),
                resident: true,
                shim: Some(sidecars.id().to_string()),
                fingerprint: live.clone(),
                turn_at: None,
                queued: queued.clone(),
            });
            // Split exactly as an attach splits it, so a harness given a backlog
            // inherits it under the same rules the real thing applies.
            let Inherited { delivered_ahead, queue } = inherited_queue(SESSION, &queued, live.as_ref());

            let shared = Arc::new(Mutex::new(ResidentState {
                phase: Phase::Starting,
                stalled: false,
                queued: delivered_ahead.len() + queue.len(),
                queued_messages: delivered_ahead
                    .iter()
                    .map(|pending| pending.preview(true))
                    .chain(queue.iter().map(|pending| pending.preview(false)))
                    .collect(),
                background: 0,
                tasks: Vec::new(),
                awaiting_background: false,
                turn_started_at: Some(iso8601(SystemTime::now())),
                question: None,
            }));
            let (tx, rx) = mpsc::unbounded_channel();
            registry.residents.lock().expect("residents").insert(
                SESSION.to_string(),
                Handle {
                    tx: tx.clone(),
                    shared: shared.clone(),
                },
            );
            let recorder = registry.recorders.open(SESSION);
            let driver = tokio::spawn(
                Driver {
                    registry: registry.clone(),
                    id: SESSION.to_string(),
                    sidecars: sidecars.clone(),
                    link,
                    record: "record-under-test".to_string(),
                    recorder,
                    live,
                    recipe: Some(recipe(&bin, &spawns, &after, recipe_fingerprint.unwrap_or_default())),
                    tx: tx.clone(),
                    rx,
                    shared: shared.clone(),
                    claim: None,
                    queue,
                    delivered_ahead,
                    absorb: AbsorbWatch::default(),
                    turn: None,
                    ready: false,
                    heard: Instant::now(),
                    replaying,
                    replay_open: false,
                    replay_marked: false,
                    pending_turn: None,
                    idle_since: None,
                    seq: 0,
                    fatal: None,
                    background: BackgroundTasks::default(),
                    awaiting_background: false,
                    awaiting_since: None,
                    deaths: VecDeque::new(),
                    rung: 0,
                    recovering: false,
                    retiring: false,
                    limit_hint: limits::HintSlot::default(),
                    ahead_noted: None,
                    // Read back out of the journal, the way an attach reads it,
                    // so a held queue is restored by the same round trip.
                    hold: registry
                        .journal
                        .read()
                        .into_iter()
                        .find(|record| record.run_id == "record-under-test")
                        .and_then(|record| record.hold),
                    asked: Vec::new(),
                    replay_asked: Vec::new(),
                    replay_refusals: Vec::new(),
                    replay_declines: Vec::new(),
                    replay_gapped: false,
                    draft: registry.draft_writer(SESSION),
                    phase_moved: std::sync::atomic::AtomicBool::new(false),
                }
                .run(),
            );
            Harness {
                _home: home,
                registry,
                tx,
                shared,
                log,
                go,
                bin,
                spawns,
                after,
                shim,
                driver,
            }
        }

        /// The ordinary harness: nothing stalls, nothing ticks in the test's
        /// lifetime, and the ladder is fast enough to watch.
        async fn ordinary() -> Harness {
            Harness::ordinary_with(hold_until_released()).await
        }

        /// The ordinary timings around a different `claude`.
        async fn ordinary_with(body: &str) -> Harness {
            Harness::start(body, Duration::from_secs(3600), Duration::from_secs(3600), Duration::from_millis(120)).await
        }

        /// A message that fits the running process, and could start another.
        fn send(&self, text: &str) {
            self.send_with(text, self.recipe(Fingerprint::default()));
        }

        /// The recipe a message carries, pointing at this harness' own `claude`.
        fn recipe(&self, fingerprint: Fingerprint) -> Recipe {
            recipe(&self.bin, &self.spawns, &self.after, fingerprint)
        }

        /// Overwrite the `claude` the next respawn will find.
        fn plan_claude(&self, body: &str) {
            write_claude(&self.bin, body);
        }

        /// Make every spawn from here on fail after it has started.
        fn refuse_to_start(&self) {
            std::fs::write(self.bin.join("refuse"), b"").expect("refuse");
        }

        /// One entry per process the driver started for itself, argv and all.
        fn spawns(&self) -> Vec<String> {
            std::fs::read_to_string(&self.spawns)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        /// What the journal says the process behind this session was started
        /// with — the thing the *next* server would read.
        ///
        /// Found by session rather than by run id, so it follows a respawn to
        /// the record that replaced the one the harness started with.
        fn recorded_fingerprint(&self) -> Option<Fingerprint> {
            self.registry
                .journal
                .read()
                .into_iter()
                .find(|record| record.session_id == SESSION)
                .and_then(|record| record.fingerprint)
        }

        /// Every line a replacement process read off its stdin.
        fn after(&self) -> Vec<String> {
            std::fs::read_to_string(&self.after)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        /// A message that wants the process `recipe` describes.
        fn send_with(&self, text: &str, recipe: Recipe) {
            self.post(Pending::new(text, recipe));
        }

        /// Put a message in the driver's inbox and walk away, without waiting
        /// for it to be written down: these tests poll for what they expect.
        /// [`a_message_sent_behind_a_turn_is_on_disk_before_its_send_is_answered`]
        /// is the one that goes through [`InjectionRegistry::deliver`] instead.
        fn post(&self, pending: Pending) {
            let (recorded, _) = tokio::sync::oneshot::channel();
            self.tx
                .send(Cmd::Deliver {
                    pending: Box::new(pending),
                    recorded,
                })
                .expect("the driver is listening");
        }

        /// A message a client named, sent the way the send route sends one:
        /// the id claimed and answered first, the message delivered under the
        /// queue id the claim chose. Returns that queue id.
        fn named(&self, text: &str) -> String {
            let receipts = self.registry.receipts();
            let client = uuid::Uuid::new_v4().to_string();
            let now = crate::inject::receipts::now_ms();
            let qid = match receipts
                .claim(&client, crate::inject::receipts::Kind::Message, Source::Claude, Some(SESSION), now)
                .expect("claim")
            {
                crate::inject::receipts::Claim::Owned { qid } => qid,
                other => panic!("a fresh id is owned, got {other:?}"),
            };
            receipts.complete(&client, "{}", None, now);
            self.post(Pending::with_id(qid.clone(), text, self.recipe(Fingerprint::default())));
            qid
        }

        /// The receipt of the named message travelling as `qid`.
        fn receipt(&self, qid: &str) -> Option<ReceiptStatus> {
            self.registry
                .receipts()
                .for_session(Source::Claude, SESSION, crate::inject::receipts::now_ms())
                .into_iter()
                .find(|receipt| receipt.qid.0.as_deref() == Some(qid))
                .map(|receipt| receipt.status)
        }

        /// Let the fake claude finish the turn it is holding.
        fn release(&self) {
            std::fs::write(&self.go, b"").expect("go");
        }

        /// Let [`wakes_itself_for_a_finished_task`] take its next step.
        fn nudge(&self, step: &str) {
            let home = self.go.parent().expect("the go file has a directory");
            std::fs::write(home.join(step), b"").expect("nudge");
        }

        /// This driver's journal record, as the next server would read it.
        fn record(&self) -> runs::RunRecord {
            self.registry
                .journal
                .read()
                .into_iter()
                .find(|record| record.run_id == "record-under-test")
                .expect("the journal still holds this driver's record")
        }

        fn state(&self) -> ResidentState {
            self.shared.lock().expect("mirror").clone()
        }

        /// The queue as a row would show it: steered first, then waiting.
        fn queued(&self) -> Vec<crate::sessions::QueuedMessage> {
            self.state().queued_messages
        }

        /// The id of the queued message whose text contains `needle`.
        fn qid(&self, needle: &str) -> String {
            self.queued()
                .into_iter()
                .find(|message| message.text.contains(needle))
                .unwrap_or_else(|| panic!("no queued message matching {needle}: {:?}", self.queued()))
                .id
        }

        async fn steer(&self, qid: &str) -> QueueOutcome {
            self.registry.steer_queued(SESSION, qid).await
        }

        async fn withdraw(&self, qid: &str) -> QueueOutcome {
            self.registry.drop_queued(SESSION, qid).await
        }

        fn injection(&self) -> Option<crate::sessions::Injection> {
            self.registry.injection_for(Source::Claude, SESSION)
        }

        fn injection_for(&self, id: &str) -> Option<crate::sessions::Injection> {
            self.registry.injection_for(Source::Claude, id)
        }

        /// Every line the fake claude has read, in the order it read them.
        fn seen(&self) -> Vec<String> {
            std::fs::read_to_string(&self.log)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        /// This session's flight recording, in the order it was written.
        fn recording(&self) -> Vec<crate::record::Event> {
            let paths = Paths::new(self._home.path().to_path_buf());
            let dirs = InstanceDirs::new(&paths.data_dir(), crate::instance::DEFAULT_INSTANCE).expect("valid name");
            crate::record::read_log(&dirs.session_logs().join(format!("{SESSION}.ndjson")))
                .unwrap_or_default()
                .into_iter()
                .map(|line| line.event)
                .collect()
        }

        /// Where the transcript of this session would be if a real claude were
        /// writing one, and the directory its subagents write into.
        fn transcript(&self) -> PathBuf {
            transcript_path(self._home.path())
        }
    }

    /// The transcript path for [`SESSION`] under a harness home.
    ///
    /// A free function because the harness needs it before there is a harness:
    /// the fake claude's script has to name it.
    fn transcript_path(home: &Path) -> PathBuf {
        let paths = Paths::new(home.to_path_buf());
        let project = paths.claude_managed_projects().join("-w-repo");
        std::fs::create_dir_all(&project).expect("project dir");
        project.join(format!("{SESSION}.jsonl"))
    }

    /// Whether a recording holds a lifecycle line for this stage.
    fn staged(events: &[crate::record::Event], stage: crate::record::Stage) -> bool {
        events
            .iter()
            .any(|event| matches!(event, crate::record::Event::Lifecycle { event, .. } if *event == stage))
    }

    /// Poll until `check` passes, or fail rather than hang forever.
    async fn until(what: &str, mut check: impl FnMut() -> bool) {
        for _ in 0..400 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("timed out waiting for {what}");
    }

    // ------------------------------------------------------- durability

    /// A message sent behind a running turn is in the journal by the time its
    /// send is answered.
    ///
    /// The send route says `200` the moment [`InjectionRegistry::deliver`]
    /// returns, and a server killed the instant after hands the next one
    /// nothing but the journal. So the journal is read here straight after the
    /// call returns, without polling: polling would wait for the driver and
    /// hide the window this is about.
    ///
    /// On the single-threaded test runtime on purpose. Nothing else runs until
    /// this task yields, so a `deliver` that posted to the driver's inbox and
    /// returned — which is what it used to do — finds the journal without the
    /// message every time, rather than whenever a busy machine lets the race go
    /// the wrong way.
    #[tokio::test]
    async fn a_message_sent_behind_a_turn_is_on_disk_before_its_send_is_answered() {
        let harness = Harness::ordinary().await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("first");
        until("the first to be taken into a turn", || {
            harness.state().phase == Phase::Working && harness.record().turn_at.is_some()
        })
        .await;

        let delivery = harness
            .registry
            .deliver(SESSION, "second", harness.recipe(Fingerprint::default()), None)
            .await
            .expect("the running resident takes it");
        let record = harness.record();
        assert!(delivery.queued, "it waits behind the turn in flight");
        assert_eq!(
            record.queued.iter().map(|queued| queued.text.as_str()).collect::<Vec<_>>(),
            vec!["second"],
            "the accepted message is on disk when the send is answered, not merely in the driver's inbox: {record:?}"
        );
        assert!(record.turn_at.is_some(), "behind the turn that is still in flight: {record:?}");

        // And it is still a queue that moves: the turn ends, and it goes in.
        harness.release();
        until("the second to reach claude", || harness.seen().iter().any(|line| line.contains("second"))).await;
        harness.release();
    }

    // ------------------------------------------------------ receipts

    /// A named message's receipt follows it: queued while it waits behind a
    /// turn, sent the moment a turn takes it — and the message sent into an
    /// idle session is sent at once.
    #[tokio::test]
    async fn a_named_messages_receipt_follows_it_from_the_queue_into_its_turn() {
        let harness = Harness::ordinary().await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        let first = harness.named("first");
        until("the first to be taken into a turn", || {
            harness.receipt(&first) == Some(ReceiptStatus::Sent)
        })
        .await;
        assert_eq!(harness.state().phase, Phase::Working);

        let second = harness.named("second");
        until("the second to wait", || harness.state().queued == 1).await;
        assert_eq!(harness.receipt(&second), Some(ReceiptStatus::Queued), "behind a turn, it is only queued");

        harness.release();
        until("the second to be taken into its turn", || {
            harness.receipt(&second) == Some(ReceiptStatus::Sent)
        })
        .await;
        assert_eq!(harness.receipt(&first), Some(ReceiptStatus::Sent), "and the first stays sent");
        harness.release();
    }

    /// Withdrawn is failed — the server dropped it unsent — and steered is
    /// sent: it is in claude's stdin, whichever turn ends up answering it.
    #[tokio::test]
    async fn a_named_message_withdrawn_is_failed_and_one_steered_is_sent() {
        let harness = Harness::ordinary().await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("busy");
        until("the turn", || harness.state().phase == Phase::Working).await;

        let dropped = harness.named("withdrawn");
        let steered = harness.named("steered");
        until("both to wait", || harness.state().queued == 2).await;
        assert_eq!(harness.withdraw(&dropped).await, QueueOutcome::Done);
        assert_eq!(harness.receipt(&dropped), Some(ReceiptStatus::Failed));
        assert_eq!(harness.steer(&steered).await, QueueOutcome::Done);
        assert_eq!(harness.receipt(&steered), Some(ReceiptStatus::Sent));
        harness.release();
    }

    /// The queue a session that cannot be brought back leaves behind is
    /// dropped — "N queued message(s) were not delivered" — and a named
    /// message in it says failed rather than queued for ever.
    #[tokio::test]
    async fn a_named_message_left_in_a_dropped_queue_is_failed() {
        let harness = Harness::ordinary_with(die_when_released()).await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.refuse_to_start();
        harness.send("doomed");
        until("the turn", || harness.state().phase == Phase::Working).await;
        // More than the recovery ladder has rungs: each replacement takes the
        // head of the queue into a turn of its own before it dies, so the last
        // of these is never written anywhere.
        for n in 0..4 {
            harness.send(&format!("filler {n}"));
        }
        let left = harness.named("left behind");
        until("the queue", || harness.state().queued == 5).await;
        assert_eq!(harness.receipt(&left), Some(ReceiptStatus::Queued));
        harness.release();

        until("the breaker to trip", || harness.driver.is_finished()).await;
        assert_eq!(harness.receipt(&left), Some(ReceiptStatus::Failed));
        let error = harness.injection().expect("a run").error.0.expect("a reason");
        assert!(error.contains("were not delivered"), "{error}");
    }

    // ------------------------------------------------------ the recording

    /// One ordinary turn, as [`crate::record`] writes it down.
    ///
    /// The sequence is the contract replay depends on: what went in, what came
    /// back, where the turn opened and closed, and — the part no stdio stream
    /// carries — what the CLI appended to its transcript files while it worked,
    /// a subagent that appeared mid-turn included.
    #[tokio::test]
    async fn a_session_records_everything_the_replay_will_need() {
        use crate::record::Event;

        let harness = Harness::ordinary().await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        // A transcript, as a real claude would be writing one. The fake cannot:
        // it is a shell script that speaks stream-json, and the two halves of a
        // session are written by the CLI independently.
        let transcript = harness.transcript();
        std::fs::write(&transcript, "{\"type\":\"user\",\"n\":1}\n").expect("transcript");

        harness.send("first");
        until("the turn", || harness.state().phase == Phase::Working).await;
        std::fs::write(&transcript, "{\"type\":\"user\",\"n\":1}\n{\"type\":\"assistant\",\"n\":2}\n").expect("transcript");
        // …and a subagent that only exists because this turn started one.
        let agent = crate::sessions::transcript::subagent_path(&transcript, "a1");
        std::fs::create_dir_all(agent.parent().expect("subagents")).expect("mkdir");
        std::fs::write(&agent, "{\"type\":\"assistant\",\"isSidechain\":true}\n").expect("agent transcript");

        harness.release();
        until("the result", || harness.state().phase == Phase::Idle).await;
        until("the transcripts to be picked up", || {
            let events = harness.recording();
            events
                .iter()
                .any(|event| matches!(event, Event::Transcript { file, .. } if file == "agent:a1"))
                && events
                    .iter()
                    .filter(|event| matches!(event, Event::Transcript { file, .. } if file == "main"))
                    .count()
                    == 2
        })
        .await;

        let events = harness.recording();
        // The prompt, exactly as claude read it.
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::Stdin { line } if line.contains("\"first\""))),
            "the message that opened the turn is in the recording: {events:?}"
        );
        // And what came back, including the preamble junk a tolerant parser
        // steps over — a recording is what was *said*, not what was understood.
        let stdout: Vec<&String> = events
            .iter()
            .filter_map(|event| match event {
                Event::Stdout { line, replayed: false } => Some(line),
                _ => None,
            })
            .collect();
        assert!(stdout.iter().any(|line| line.contains("\"subtype\":\"init\"")), "{stdout:?}");
        assert!(stdout.iter().any(|line| line.contains("this line is not json")), "{stdout:?}");
        assert!(stdout.iter().any(|line| line.contains("\"type\":\"result\"")), "{stdout:?}");

        assert!(staged(&events, crate::record::Stage::TurnOpen));
        assert!(staged(&events, crate::record::Stage::Result));
        // Order, not just presence: a turn opens before it ends, and the prompt
        // goes in before the answer comes back.
        let position = |wanted: fn(&Event) -> bool| events.iter().position(wanted).expect("an expected event");
        let opened = position(|event| matches!(event, Event::Lifecycle { event, .. } if *event == crate::record::Stage::TurnOpen));
        let ended = position(|event| matches!(event, Event::Lifecycle { event, .. } if *event == crate::record::Stage::Result));
        let prompt = position(|event| matches!(event, Event::Stdin { line } if line.contains("\"first\"")));
        assert!(prompt < opened || opened < ended, "the turn's line order is wrong: {events:?}");
        assert!(opened < ended, "a turn ends after it opens: {events:?}");

        // The transcript lines are the CLI's own, verbatim and in order.
        let main: Vec<&String> = events
            .iter()
            .filter_map(|event| match event {
                Event::Transcript { file, line, .. } if file == "main" => Some(line),
                _ => None,
            })
            .collect();
        assert_eq!(main, ["{\"type\":\"user\",\"n\":1}", "{\"type\":\"assistant\",\"n\":2}"], "{main:?}");
    }

    // ------------------------------------------------------ the state machine

    /// A turn claude opens by itself is still a turn.
    ///
    /// The reported bug, from the top: a background task finished, its
    /// notification woke the main agent, and the CLI ran a whole turn with
    /// nothing on stdin to have started it. The session showed *idle* the whole
    /// time, because the phase only ever moved when this server wrote a prompt.
    ///
    /// The three lines that must **not** move it are asserted first, because
    /// each of them is a way to get a session stuck working on nothing: a
    /// subagent's forwarded stream, a background task's progress, and the woken
    /// turn's own `init`.
    #[tokio::test]
    async fn a_turn_claude_opens_for_itself_takes_the_session_out_of_idle() {
        let harness = Harness::ordinary_with(&wakes_itself_for_a_finished_task()).await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        // A subagent talking is not this session working. The task list that
        // follows it is the barrier: seeing it move proves the driver has read
        // everything above it.
        harness.nudge("subagent");
        until("the subagent's stream", || harness.state().background == 1).await;
        assert_eq!(
            harness.state().phase,
            Phase::Idle,
            "a forwarded child event is the subagent working, not this thread"
        );

        // Nor is the `init` the woken turn announces itself with — every process
        // writes one of those at start-up.
        harness.nudge("wake");
        until("the woken turn's init", || harness.state().background == 0).await;
        assert_eq!(harness.state().phase, Phase::Idle, "an init is not a turn");

        // The main thread's own first line is.
        harness.nudge("speak");
        until("the turn claude opened", || harness.state().phase == Phase::Working).await;

        // And claude's own `result` ends it, exactly as it ends a turn this
        // server wrote a prompt for.
        harness.nudge("finish");
        until("the result", || harness.state().phase == Phase::Idle).await;

        assert!(
            harness.seen().is_empty(),
            "nothing was written to claude for a turn it started itself: {:?}",
            harness.seen()
        );
        // The recording says which kind of turn it was, so the next person
        // reading a flight log can tell a turn with no `stdin` line above it
        // from a bug.
        until("the recording to name the implicit turn", || {
            harness.recording().iter().any(|event| {
                matches!(
                    event,
                    crate::record::Event::Lifecycle {
                        event: crate::record::Stage::TurnOpen,
                        detail: Some(detail),
                        ..
                    } if detail.contains("a turn of its own")
                )
            })
        })
        .await;
    }

    /// The other half of the same bug, and the reason it was worth fixing.
    ///
    /// A message sent during a turn claude opened for itself used to be judged
    /// as arriving between turns, so it went straight into stdin — where the CLI
    /// folded it into the reply already running and it never became a `user`
    /// entry at all. With the turn accounted for, the queue machinery that
    /// already existed takes it: journalled on arrival, written when the turn
    /// claude opened ends, and answered in a turn of its own.
    #[tokio::test]
    async fn a_message_sent_during_a_turn_claude_opened_waits_for_it() {
        let harness = Harness::ordinary_with(&wakes_itself_for_a_finished_task()).await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.nudge("subagent");
        harness.nudge("wake");
        harness.nudge("speak");
        until("the turn claude opened", || harness.state().phase == Phase::Working).await;

        harness.send("所以结论呢？");
        until("the message to be queued", || harness.state().queued == 1).await;
        assert!(
            harness.seen().is_empty(),
            "the message was not written into the middle of the turn: {:?}",
            harness.seen()
        );
        // On disk as well as in memory: a restart in this window has to find it.
        until("the journal to hold it", || {
            harness.record().queued.iter().any(|queued| queued.text.contains("所以结论呢"))
        })
        .await;

        // The turn ends, and the queue behind it moves — into a turn of its own,
        // which is what gives the message a `user` entry and an answer.
        harness.nudge("finish");
        until("the message to reach claude in its own turn", || {
            harness.state().phase == Phase::Working && harness.seen().iter().any(|line| line.contains("所以结论呢"))
        })
        .await;
        harness.release();
        until("the queue to drain", || {
            harness.state().queued == 0 && harness.state().phase == Phase::Idle
        })
        .await;
    }

    /// A claude that writes the `allowed` event before every turn's result, as
    /// the real one does.
    fn reports_its_windows() -> String {
        format!(
            r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
    *control_request*) : ;;
    *) printf '%s\n' '{ALLOWED_WITH_WINDOWS}'
       printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}' ;;
  esac
done
"#
        )
    }

    /// The live path end to end: a session on the local login writes an
    /// `allowed` event, and the panel's reading of the local login has it.
    #[tokio::test]
    async fn every_rate_limit_event_is_filed_under_the_login_it_ran_on() {
        let harness = Harness::ordinary_with(&reports_its_windows()).await;
        until("the init to land", || harness.state().phase == Phase::Idle).await;
        harness.send("first");
        let file = harness.registry.paths.data_dir().join("local-usage.json");
        until("the reading to be filed", || file.is_file()).await;

        let reading = harness.registry.quota.cached(&crate::quota::UsageLogin::Local);
        let windows: Vec<(&str, f64)> = reading.windows.iter().map(|w| (w.label.as_str(), w.used_percent)).collect();
        assert_eq!(windows, [("5h", 19.0), ("weekly", 46.0)]);
        assert_eq!(reading.source, Some(crate::quota::QuotaReadingSource::Session));
        assert!(reading.updated_at.is_some());
    }

    /// The whole cycle, twice, on one process — which is the entire point of
    /// the change. Under the old model the second message would have been a
    /// second `claude`.
    #[tokio::test]
    async fn a_turn_runs_from_init_to_result_and_the_process_lives_on() {
        let harness = Harness::ordinary().await;
        until("the init to land", || harness.state().phase == Phase::Idle).await;
        assert!(harness.injection().is_none(), "an idle resident has no run to report");

        harness.send("first");
        until("the turn to open", || harness.state().phase == Phase::Working).await;
        assert_eq!(
            harness.injection().expect("a run").state,
            InjectionRunState::Running,
            "a turn in flight is what the composer sees"
        );

        harness.release();
        until("the result", || harness.state().phase == Phase::Idle).await;
        let injection = harness.injection().expect("a run");
        assert_eq!(injection.state, InjectionRunState::Ok);
        assert!(injection.finished_at.0.is_some(), "and it has an end time");

        // The same process takes the next one.
        harness.send("second");
        until("the second turn", || harness.state().phase == Phase::Working).await;
        harness.release();
        until("the second result", || harness.state().phase == Phase::Idle).await;
        assert_eq!(harness.injection().expect("a run").state, InjectionRunState::Ok);

        let seen = harness.seen();
        assert_eq!(seen.len(), 2, "two messages, one process: {seen:?}");
        assert!(seen[0].contains("\"first\""), "{seen:?}");
        assert!(!harness.driver.is_finished(), "the process is still there for a third");
    }

    #[tokio::test]
    async fn uploads_directory_failure_prevents_spawning_a_misconfigured_process() {
        let harness = Harness::ordinary().await;
        let paths = &harness.registry.paths;
        std::fs::write(paths.uploads_dir(), b"not a directory").unwrap();
        let recipe = crate::inject::claude_recipe(paths, harness.bin.to_str(), &Default::default(), Env::new(), Default::default());
        let recorder = harness.registry.recorders.open(SESSION);
        let result = harness.registry.spawn_link(SESSION, &recipe, Selector::Fresh, None, &[], &recorder).await;
        assert!(matches!(result, Err(InjectError::Message(ref message)) if message.contains("cannot prepare the uploads directory")));
        assert!(harness.spawns().is_empty());
    }

    #[tokio::test]
    async fn text_then_attachments_keep_the_upload_enabled_resident() {
        let harness = Harness::ordinary().await;
        until("the original init", || harness.state().phase == Phase::Idle).await;
        assert!(!harness.registry.paths.uploads_dir().exists());
        // A real spawn through the production recipe, leaving background work
        // alive between turns so a spurious settings change would park or kill it.
        let script = replacement_claude(SESSION, FORKED).replace(
            "*) printf",
            "*) printf '%s\\n' '{\"type\":\"system\",\"subtype\":\"task_started\",\"task_id\":\"uploads-task\",\"description\":\"still working\"}'; printf",
        );
        harness.plan_claude(&script);
        let make_recipe = |options: &crate::inject::SendOptions| {
            crate::inject::claude_recipe(
                &harness.registry.paths,
                harness.bin.to_str(),
                options,
                harness.recipe(Fingerprint::default()).env,
                Default::default(),
            )
        };
        harness.send_with("text first", make_recipe(&Default::default()));
        until("the upload-enabled process to finish the first turn", || {
            harness.after().len() == 1 && harness.state().phase == Phase::Idle
        })
        .await;
        assert!(harness.registry.paths.uploads_dir().is_dir());
        assert!(harness.recorded_fingerprint().unwrap().uploads);
        let first = harness.registry.journal.read().into_iter().find(|r| r.session_id == SESSION).unwrap();
        assert_eq!(harness.spawns().len(), 1);
        assert!(harness.spawns()[0].contains("--add-dir"));
        for (i, options) in [
            crate::inject::SendOptions {
                images: vec!["/uploads/screenshot.png".into()],
                ..Default::default()
            },
            crate::inject::SendOptions {
                files: vec!["/uploads/log.txt".into()],
                ..Default::default()
            },
            crate::inject::SendOptions::default(),
        ]
        .into_iter()
        .enumerate()
        {
            let text = crate::inject::with_claude_attachments("next message", &options);
            harness.send_with(&text, make_recipe(&options));
            until("the same process to receive the next message", || {
                harness.after().len() == i + 2 && harness.state().phase == Phase::Idle
            })
            .await;
            let current = harness.registry.journal.read().into_iter().find(|r| r.session_id == SESSION).unwrap();
            assert_eq!(current.cli_pid, first.cli_pid);
            assert_eq!(current.run_id, first.run_id);
            assert_eq!(harness.spawns().len(), 1);
            assert!(!harness.state().awaiting_background);
            assert_eq!(harness.state().background, 1);
        }
        assert!(harness.after()[1].contains("screenshot.png"));
        assert!(harness.after()[2].contains("log.txt"));
    }

    /// The behaviour of the real CLI that every other test's fake glosses
    /// over: claude 2.1.231 given a session selector says *nothing* — not even
    /// `init` — until it has read a first message. A driver that waits for
    /// `init` to deliver deadlocks such a process for ever, which is exactly
    /// what happened the first time this module met a real claude.
    #[tokio::test]
    async fn a_claude_that_says_nothing_until_spoken_to_still_gets_the_message() {
        let harness = Harness::start_with_handshake(
            "", // no init up front: the body speaks only once it has read
            &format!(
                r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'
  printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}'
done
"#
            ),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
        )
        .await;
        harness.send("wake up");
        until("the whole turn to run on a taciturn claude", || {
            harness.injection().is_some_and(|run| run.state == InjectionRunState::Ok)
        })
        .await;
        assert!(harness.seen()[0].contains("wake up"), "{:?}", harness.seen());
    }

    /// The queue is FIFO and one-at-a-time: a message sent mid-turn waits, and
    /// nothing reaches claude until the turn before it has ended.
    #[tokio::test]
    async fn messages_sent_during_a_turn_are_delivered_in_order_afterwards() {
        let harness = Harness::ordinary().await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send("one");
        until("the first turn", || harness.state().phase == Phase::Working).await;
        harness.send("two");
        harness.send("three");
        until("both to be queued", || harness.state().queued == 2).await;
        assert_eq!(harness.seen().len(), 1, "nothing jumps the turn in flight");

        // One release per turn, each only once the turn before it has actually
        // been picked up: the fake claude holds exactly one turn at a time, and
        // releasing twice into the same one would prove nothing about order.
        harness.release();
        until("two to be read", || harness.seen().len() == 2).await;
        harness.release();
        until("three to be read", || harness.seen().len() == 3).await;
        harness.release();
        until("the queue to drain", || {
            harness.state().queued == 0 && harness.state().phase == Phase::Idle
        })
        .await;

        let seen = harness.seen();
        assert_eq!(seen.len(), 3);
        for (index, expected) in ["one", "two", "three"].iter().enumerate() {
            assert!(seen[index].contains(&format!("\"{expected}\"")), "out of order: {seen:?}");
        }
    }

    /// The message really is stream-json on stdin, in the shape claude parses.
    #[tokio::test]
    async fn a_message_reaches_claude_as_one_stream_json_user_line() {
        let harness = Harness::ordinary().await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("hello \"world\"");
        until("the message", || !harness.seen().is_empty()).await;

        let line = harness.seen().remove(0);
        let value: Value = serde_json::from_str(&line).expect("stdin carries one JSON object per line");
        assert_eq!(value["type"], "user");
        assert_eq!(value["message"]["role"], "user");
        assert_eq!(value["message"]["content"][0]["type"], "text");
        assert_eq!(value["message"]["content"][0]["text"], "hello \"world\"");
    }

    // ----------------------------------------------------------- the queue

    /// What a row can now say about a queue, which used to be a number.
    #[tokio::test]
    async fn every_queued_message_is_named_and_previewed() {
        let harness = Harness::ordinary().await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send("one");
        until("the first turn", || harness.state().phase == Phase::Working).await;
        assert!(harness.queued().is_empty(), "the message in flight is a turn, not a queue");

        harness.send(&"long ".repeat(100));
        harness.send("two");
        until("both to be queued", || harness.state().queued == 2).await;

        let queued = harness.queued();
        assert_eq!(queued.len(), 2, "{queued:?}");
        assert_eq!(queued[1].text, "two", "the queue is in the order it will run");
        assert_eq!(
            queued[0].text.chars().count(),
            QUEUE_PREVIEW_CAP + 1,
            "a long prompt is previewed, ellipsis included: {:?}",
            queued[0].text
        );
        assert!(queued[0].text.ends_with('…'));
        assert!(queued.iter().all(|message| !message.steered), "nothing was steered: {queued:?}");
        assert_ne!(queued[0].id, queued[1].id, "two messages are two ids");

        // The ids outlive the wait, and disappear with the message rather than
        // being handed on to whatever takes its place.
        let first = queued[0].id.clone();
        harness.release();
        until("the long one to start", || harness.state().queued == 1).await;
        assert!(
            harness.queued().iter().all(|message| message.id != first),
            "a message that has started is no longer in the queue"
        );
    }

    /// The point of the whole mechanism: a correction that cannot wait goes into
    /// claude's stdin now, and therefore runs before messages sent before it.
    ///
    /// The fake claude models the CLI behaviour this rests on — it reads its
    /// stdin one line at a time, so a line written mid-turn sits in the pipe
    /// until the turn it interrupted has finished. That is exactly what claude
    /// 2.1.231 does with a `user` line written under stream-json.
    #[tokio::test]
    async fn a_steered_message_runs_before_the_queue_it_jumped() {
        let harness = Harness::ordinary().await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send("one");
        until("the turn", || harness.state().phase == Phase::Working).await;
        harness.send("two");
        harness.send("urgent");
        until("both to be queued", || harness.state().queued == 2).await;

        let urgent = harness.qid("urgent");
        assert_eq!(harness.steer(&urgent).await, QueueOutcome::Done);

        // Still outstanding — it has not run — but no longer withdrawable, and
        // the row says which of the two it is.
        assert_eq!(harness.state().queued, 2, "a steered message is still waiting to run");
        let queued = harness.queued();
        assert_eq!(queued[0].id, urgent, "it runs first now: {queued:?}");
        assert!(queued[0].steered, "{queued:?}");
        assert!(!queued[1].steered, "and the message it jumped is untouched: {queued:?}");
        assert_eq!(harness.seen().len(), 1, "the turn in flight was not interrupted or re-prompted");

        // The turn in flight ends, and claude answers what is in its stdin
        // before anything this driver has left to write.
        harness.release();
        until("the steered message to be read", || harness.seen().len() == 2).await;
        assert!(harness.seen()[1].contains("\"urgent\""), "{:?}", harness.seen());
        // Both halves, because either alone is ambiguous: the process reads the
        // steered line as soon as its own turn ends, which is concurrent with
        // this driver noticing the `result`, so `Working` on its own can still
        // be the *previous* turn. `Working` with one message left is only ever
        // the steered one running.
        until("the steered message's turn to open", || {
            let state = harness.state();
            state.phase == Phase::Working && state.queued == 1
        })
        .await;
        let queued = harness.queued();
        assert_eq!(queued.len(), 1, "the steered message is running, not queued: {queued:?}");
        assert_eq!(queued[0].text, "two", "what is left is the message it jumped");

        harness.release();
        until("the message it jumped to be read", || harness.seen().len() == 3).await;
        assert!(harness.seen()[2].contains("\"two\""), "{:?}", harness.seen());
        harness.release();
        until("the queue to drain", || {
            harness.state().queued == 0 && harness.state().phase == Phase::Idle
        })
        .await;
        assert_eq!(
            harness.injection().expect("a run").state,
            InjectionRunState::Ok,
            "every turn ended normally"
        );
        assert!(staged(&harness.recording(), crate::record::Stage::Steer), "the recording says why");
    }

    /// The session-killer, reproduced: claude answers the steered message
    /// *inside* the turn it was written into, and says nothing more.
    ///
    /// What used to happen here is the whole reason this test exists. The
    /// driver opened a turn for the steered message on the strength of its own
    /// bookkeeping, no `result` for it ever came — the CLI had already answered
    /// it — and the session showed `working` for ever with the ordinary queue
    /// pinned behind a turn nothing could end. Only Stop got out of it.
    ///
    /// Now the turn is provisional, the silence is the answer, and the queue
    /// moves on its own.
    #[tokio::test]
    async fn a_steered_message_the_turn_swallowed_does_not_wedge_the_queue() {
        let settle = Duration::from_millis(200);
        let harness = Harness::settling(folds_the_steered_message(), settle, Duration::from_millis(120)).await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send("slow");
        until("the turn", || harness.state().phase == Phase::Working).await;
        harness.send("after");
        harness.send("urgent");
        until("both to be queued", || harness.state().queued == 2).await;

        let urgent = harness.qid("urgent");
        assert_eq!(harness.steer(&urgent).await, QueueOutcome::Done);
        // The fake claude has to have *read* the steered line before the turn is
        // released, or it would answer it separately and this would be the other
        // scenario. That is exactly what the real CLI's injection window is.
        until("claude to take the steered message into the running turn", || harness.seen().len() == 2).await;

        let released = Instant::now();
        harness.release();

        // One `result`, covering both prompts. The queue behind it must move
        // anyway, and within a window a person would call automatic.
        until("the message it jumped to start on its own", || {
            harness.seen().iter().any(|line| line.contains("\"after\""))
        })
        .await;
        assert!(
            released.elapsed() < Duration::from_secs(3),
            "the queue waited {:?} on a turn that had already been answered",
            released.elapsed()
        );
        let state = harness.state();
        assert_eq!(state.phase, Phase::Working, "the message it jumped is running: {state:?}");
        assert_eq!(state.queued, 0, "and nothing is left waiting: {state:?}");

        harness.release();
        until("the session to go quiet", || {
            harness.state().queued == 0 && harness.state().phase == Phase::Idle
        })
        .await;
        assert_eq!(
            harness.injection().expect("a run").state,
            InjectionRunState::Ok,
            "no turn was left running and none was failed"
        );
        assert_eq!(harness.seen().len(), 3, "and nothing was written twice: {:?}", harness.seen());
        assert!(
            staged(&harness.recording(), crate::record::Stage::Absorb),
            "the recording says which of the two things the CLI did: {:?}",
            harness.recording()
        );

        // The ghost, stated as the assertion it is: `Working` outlives nothing.
        tokio::time::sleep(settle * 3).await;
        assert_eq!(harness.state().phase, Phase::Idle, "no turn is left running");
    }

    /// The other thing the same CLI does: the steered message is buffered and
    /// answered as a turn of its own, `system:init` and all.
    ///
    /// The half that must not regress while the half above is fixed, and the
    /// settle window is deliberately short here to prove it cannot: the
    /// deferred turn announces itself and is then held for many times the
    /// window, so a driver that judged on silence alone would close it early —
    /// and the `result` that eventually arrived would land on the *next*
    /// message's turn, failing it for something it never did.
    #[tokio::test]
    async fn a_steered_message_claude_answers_separately_keeps_its_own_turn() {
        let settle = Duration::from_millis(120);
        let harness = Harness::settling(&defers_the_steered_message(), settle, Duration::from_millis(120)).await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send("slow");
        until("the turn", || harness.state().phase == Phase::Working).await;
        harness.send("after");
        harness.send("urgent");
        until("both to be queued", || harness.state().queued == 2).await;

        let urgent = harness.qid("urgent");
        assert_eq!(harness.steer(&urgent).await, QueueOutcome::Done);
        harness.release();

        // claude reads the steered line, announces a turn for it, and holds it.
        until("the steered message to be read", || harness.seen().len() == 2).await;
        assert!(harness.seen()[1].contains("\"urgent\""), "{:?}", harness.seen());
        until("its own turn to be running", || {
            let state = harness.state();
            state.phase == Phase::Working && state.queued == 1
        })
        .await;

        // Far longer than the settle window, with nothing on stdout: the `init`
        // is what holds the turn open, and it has to be enough on its own.
        tokio::time::sleep(settle * 5).await;
        let state = harness.state();
        assert_eq!(state.phase, Phase::Working, "the deferred turn is still running: {state:?}");
        assert_eq!(state.queued, 1, "and the message behind it has not been let out: {state:?}");
        assert_eq!(harness.seen().len(), 2, "nothing jumped it: {:?}", harness.seen());

        // Its `result` ends its own turn and nobody else's, and what it was
        // holding up starts next.
        harness.release();
        until("the message it jumped to run", || harness.seen().len() == 3).await;
        assert!(harness.seen()[2].contains("\"after\""), "{:?}", harness.seen());
        harness.release();
        until("the queue to drain", || {
            harness.state().queued == 0 && harness.state().phase == Phase::Idle
        })
        .await;
        assert_eq!(
            harness.injection().expect("a run").state,
            InjectionRunState::Ok,
            "the message the steered one jumped ran to a normal end"
        );
    }

    /// The CLI as it is now: a steered message **folded** into the turn that is
    /// running, with the transcript the only thing that says so.
    ///
    /// The turn's opening is on file before anything is steered — the prompt,
    /// a tool call, and an *older* fold of the same words from earlier in the
    /// session, which must not be mistaken for this one. Once the steered line
    /// has been read and the test says `fold`, the CLI's record of the fold is
    /// appended the way a real session wrote it (`3c002577`): `enqueue`, the
    /// `queued_command` attachment, `remove`. Then the turn keeps running until
    /// released — the stretch the queue used to be wrong for.
    fn folds_into_the_transcript() -> &'static str {
        r#"
await() { n=0; while [ ! -f "$GO_DIR/$1" ] && [ "$n" -lt 400 ]; do [ -d "$GO_DIR" ] || exit 97; sleep 0.02; n=$((n + 1)); done; }
IFS= read -r first
printf '%s\n' "$first" >> "$LOG"
printf '%s\n' '{"type":"attachment","attachment":{"type":"queued_command","prompt":"urgent","commandMode":"prompt"},"timestamp":"2026-10-18T09:00:00.000Z"}' >> "$TRANSCRIPT"
printf '%s\n' '{"type":"user","message":{"role":"user","content":[{"type":"text","text":"slow"}]}}' >> "$TRANSCRIPT"
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}}]}}' >> "$TRANSCRIPT"
touch "$GO_DIR/opened"
IFS= read -r second
printf '%s\n' "$second" >> "$LOG"
await fold
printf '%s\n' '{"type":"queue-operation","operation":"enqueue","content":"urgent"}' >> "$TRANSCRIPT"
printf '%s\n' '{"type":"attachment","attachment":{"type":"queued_command","prompt":[{"text":"urgent","type":"text"}],"commandMode":"prompt"}}' >> "$TRANSCRIPT"
printf '%s\n' '{"type":"queue-operation","operation":"remove"}' >> "$TRANSCRIPT"
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"answering both"}]}}' >> "$TRANSCRIPT"
touch "$GO_DIR/folded"
await_go
rm -f "$GO"
printf '%s\n' '{"type":"assistant","message":{}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
    *control_request*) : ;;
    *) await_go
       rm -f "$GO"
       printf '%s\n' '{"type":"result","subtype":"success","is_error":false}' ;;
  esac
done
"#
    }

    /// The same CLI **buffering** the steered message for a turn of its own,
    /// and writing down what it really writes for that: an `enqueue` with the
    /// words while the turn runs, then `dequeue` and an ordinary `user` entry
    /// once its own turn — announced by `init` — begins.
    fn buffers_into_the_transcript() -> String {
        format!(
            r#"
IFS= read -r first
printf '%s\n' "$first" >> "$LOG"
printf '%s\n' '{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"slow"}}]}}}}' >> "$TRANSCRIPT"
printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"t1","name":"Bash","input":{{}}}}]}}}}' >> "$TRANSCRIPT"
IFS= read -r second
printf '%s\n' "$second" >> "$LOG"
printf '%s\n' '{{"type":"queue-operation","operation":"enqueue","content":"urgent"}}' >> "$TRANSCRIPT"
touch "$GO_DIR/buffered"
await_go
rm -f "$GO"
printf '%s\n' '{{"type":"assistant","message":{{}}}}'
printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}'
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'
printf '%s\n' '{{"type":"queue-operation","operation":"dequeue"}}' >> "$TRANSCRIPT"
printf '%s\n' '{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"urgent"}}]}}}}' >> "$TRANSCRIPT"
await_go
rm -f "$GO"
printf '%s\n' '{{"type":"assistant","message":{{}}}}'
printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
    *control_request*) : ;;
    *) await_go
       rm -f "$GO"
       printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}' ;;
  esac
done
"#
        )
    }

    /// Whether the fake claude has touched `step` in the harness' directory.
    fn reached(harness: &Harness, step: &str) -> bool {
        harness.go.parent().expect("the go file has a directory").join(step).exists()
    }

    /// The bug, as a user saw it: a message steered into a running turn was
    /// folded into that turn and answered there, while the queue went on saying
    /// "delivered, waiting to start" — the row, the count, the journal — until
    /// the turn ended, with a Stop hint beside it that no longer meant anything.
    ///
    /// Now the transcript's record of the fold takes it off the queue while the
    /// turn is still running. Two neighbours that must not move with it: an
    /// identical message still waiting in the queue proper (never delivered, so
    /// no fold can be its), and an older fold of the same words that was in the
    /// transcript before the steer. And the turn's end must open no turn for the
    /// folded message — with a settle nobody can wait out, a provisional one
    /// would hold everything behind it.
    #[tokio::test]
    async fn a_steer_folded_into_the_running_turn_leaves_the_queue_while_that_turn_runs() {
        let harness = Harness::start(
            folds_into_the_transcript(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
        )
        .await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send("slow");
        until("the turn to be on file", || reached(&harness, "opened")).await;
        harness.send("urgent");
        harness.send("urgent");
        until("both to be queued", || harness.state().queued == 2).await;
        let ids: Vec<String> = harness.queued().into_iter().map(|message| message.id).collect();
        let (waiting, steered) = (ids[0].clone(), ids[1].clone());
        assert_eq!(harness.steer(&steered).await, QueueOutcome::Done);
        until("claude to read the steered line", || harness.seen().len() == 2).await;

        // Read, not yet folded: claude's, and still to come. Six polls go by,
        // and the fold already in the history is not mistaken for this one.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let queued = harness.queued();
        assert_eq!(queued.len(), 2, "{queued:?}");
        assert!(queued[0].id == steered && queued[0].steered, "{queued:?}");

        harness.nudge("fold");
        until("the fold to take it off the queue", || harness.state().queued == 1).await;
        let state = harness.state();
        assert_eq!(state.phase, Phase::Working, "the turn it was folded into is still running: {state:?}");
        assert_eq!(state.queued_messages.len(), 1, "{state:?}");
        assert_eq!(
            state.queued_messages[0].id, waiting,
            "the identical message in the queue is untouched: {state:?}"
        );
        assert!(!state.queued_messages[0].steered, "and still undelivered: {state:?}");
        let journalled: Vec<_> = harness.record().queued.into_iter().map(|queued| queued.id).collect();
        assert_eq!(
            journalled,
            std::slice::from_ref(&waiting),
            "the journal no longer holds the folded one either"
        );

        // One `result`, covering both prompts; the waiting message is next.
        harness.release();
        until("the waiting message to go in", || harness.seen().len() == 3).await;
        assert!(harness.seen()[2].contains("\"urgent\""), "{:?}", harness.seen());
        assert_eq!(harness.state().queued, 0, "{:?}", harness.state());
        harness.release();
        until("the session to go quiet", || harness.state().phase == Phase::Idle).await;
        assert_eq!(harness.seen().len(), 3, "nothing was written twice: {:?}", harness.seen());
        assert_eq!(harness.injection().expect("a run").state, InjectionRunState::Ok);
        assert!(
            staged(&harness.recording(), crate::record::Stage::Absorb),
            "the recording says the message was folded: {:?}",
            harness.recording()
        );
    }

    /// A watched session does not wait for the poll: its feed, having just sent
    /// the grown transcript, tells the resident to look — so the message leaves
    /// the queue on screen in the moment it appears in the transcript.
    #[tokio::test]
    async fn the_feed_seeing_the_fold_is_enough_on_its_own() {
        let harness = Harness::build(
            &preamble(),
            folds_into_the_transcript(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
            NEVER_SETTLES,
            Adoption {
                absorb_poll: NEVER_SETTLES,
                ..Adoption::default()
            },
        )
        .await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("slow");
        until("the turn to be on file", || reached(&harness, "opened")).await;
        harness.send("urgent");
        until("it to be queued", || harness.state().queued == 1).await;
        assert_eq!(harness.steer(&harness.qid("urgent")).await, QueueOutcome::Done);
        harness.nudge("fold");
        until("the fold to be on file", || reached(&harness, "folded")).await;

        // No poll is coming, so nothing has looked yet.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(harness.state().queued, 1, "{:?}", harness.state());

        harness.registry.transcript_grew(Source::Claude, SESSION);
        until("the nudge to take it off the queue", || harness.state().queued == 0).await;
        assert_eq!(harness.state().phase, Phase::Working, "the turn is still running");
        // A nudge for a source with no residents of this kind changes nothing.
        harness.registry.transcript_grew(Source::Codex, SESSION);

        harness.release();
        until("the session to go quiet", || harness.state().phase == Phase::Idle).await;
        assert_eq!(harness.seen().len(), 2, "{:?}", harness.seen());
    }

    /// Neither poll nor nudge has come round when the turn ends — the fold was
    /// the last thing before the `result`. The turn's end reads for it once
    /// more before opening a turn for the steered message, so no provisional
    /// turn is opened for something already answered.
    #[tokio::test]
    async fn the_turns_end_catches_a_fold_nothing_else_saw() {
        let harness = Harness::build(
            &preamble(),
            folds_into_the_transcript(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
            NEVER_SETTLES,
            Adoption {
                absorb_poll: NEVER_SETTLES,
                ..Adoption::default()
            },
        )
        .await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("slow");
        until("the turn to be on file", || reached(&harness, "opened")).await;
        harness.send("urgent");
        until("it to be queued", || harness.state().queued == 1).await;
        assert_eq!(harness.steer(&harness.qid("urgent")).await, QueueOutcome::Done);
        harness.nudge("fold");
        until("the fold to be on file", || reached(&harness, "folded")).await;
        assert_eq!(harness.state().queued, 1, "nothing has looked yet");

        harness.release();
        // With a settle nobody can wait out, a provisional turn would leave the
        // session `working` for ever.
        until("the session to go quiet", || {
            let state = harness.state();
            state.phase == Phase::Idle && state.queued == 0
        })
        .await;
        assert_eq!(harness.seen().len(), 2, "{:?}", harness.seen());
        assert_eq!(harness.injection().expect("a run").state, InjectionRunState::Ok);
    }

    /// A steered message the CLI holds for a turn of its own is *not* folded,
    /// whatever the transcript says about it in the meantime: an `enqueue` with
    /// its words while the turn runs, then a `user` entry once its own turn
    /// starts. It stays queued — delivered, waiting to start — for the whole of
    /// the running turn, and gets that turn when it comes.
    #[tokio::test]
    async fn a_steer_held_for_its_own_turn_stays_queued_until_that_turn() {
        let settle = Duration::from_millis(120);
        let harness = Harness::settling(&buffers_into_the_transcript(), settle, Duration::from_millis(120)).await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("slow");
        until("the turn", || harness.state().phase == Phase::Working).await;
        harness.send("after");
        harness.send("urgent");
        until("both to be queued", || harness.state().queued == 2).await;
        assert_eq!(harness.steer(&harness.qid("urgent")).await, QueueOutcome::Done);
        until("claude to buffer it", || reached(&harness, "buffered")).await;

        tokio::time::sleep(Duration::from_millis(300)).await;
        let queued = harness.queued();
        assert_eq!(queued.len(), 2, "still waiting for a turn of its own: {queued:?}");
        assert!(queued[0].text == "urgent" && queued[0].steered, "{queued:?}");
        assert!(queued[1].text == "after" && !queued[1].steered, "{queued:?}");

        harness.release();
        until("its own turn to be running", || {
            let state = harness.state();
            state.phase == Phase::Working && state.queued == 1
        })
        .await;
        // Many times the settle window: `init` made the turn real, and the
        // `user` entry the transcript gained is not a fold.
        tokio::time::sleep(settle * 5).await;
        let state = harness.state();
        assert_eq!(state.phase, Phase::Working, "{state:?}");
        assert_eq!(state.queued_messages.len(), 1, "{state:?}");
        assert_eq!(state.queued_messages[0].text, "after", "{state:?}");
        assert_eq!(harness.seen().len(), 2, "nothing jumped it: {:?}", harness.seen());

        harness.release();
        until("the message it jumped to run", || harness.seen().len() == 3).await;
        harness.release();
        until("the queue to drain", || {
            harness.state().queued == 0 && harness.state().phase == Phase::Idle
        })
        .await;
    }

    /// Stop, pressed in the window where nobody yet knows whether the steered
    /// message has a turn of its own.
    ///
    /// Nothing about the ladder changes — the polite request goes out, the
    /// rungs are armed — but the two clocks now have to be read together. The
    /// settle is the shorter of them (two seconds against the ladder's five,
    /// and in the same ratio here), so a turn claude had already answered is
    /// closed by the settle *first*, and the rung that comes due afterwards
    /// finds no turn of that sequence and signals nothing. A session that had
    /// finished its work is not killed for having been asked to stop it.
    #[tokio::test]
    async fn stop_inside_the_settle_window_neither_panics_nor_signals() {
        let settle = Duration::from_millis(100);
        let step = Duration::from_millis(800);
        let harness = Harness::settling(folds_the_steered_message(), settle, step).await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send("slow");
        until("the turn", || harness.state().phase == Phase::Working).await;
        harness.send("urgent");
        until("it to be queued", || harness.state().queued == 1).await;
        let urgent = harness.qid("urgent");
        assert_eq!(harness.steer(&urgent).await, QueueOutcome::Done);
        until("claude to take it into the running turn", || harness.seen().len() == 2).await;

        harness.release();
        until("the provisional turn", || {
            harness.state().queued == 0 && harness.state().phase == Phase::Working
        })
        .await;
        assert!(harness.registry.interrupt_resident(SESSION), "there is a turn to stop");

        until("the session to end up idle", || harness.state().phase == Phase::Idle).await;
        // Long enough for every rung of the ladder to have come due.
        tokio::time::sleep(step * 4).await;
        assert_eq!(harness.state().phase, Phase::Idle, "nothing re-opened the turn");
        assert!(
            harness.injection().is_some_and(|run| run.state != InjectionRunState::Running),
            "and no run was left in flight: {:?}",
            harness.injection()
        );
        assert!(
            !staged(&harness.recording(), crate::record::Stage::Escalate),
            "a turn claude had already answered was not signalled: {:?}",
            harness.recording()
        );
        assert!(
            !staged(&harness.recording(), crate::record::Stage::Exit),
            "and the process outlived the stop: {:?}",
            harness.recording()
        );
    }

    /// A message that needs a *different process* cannot jump the queue while a
    /// turn is running: there is no stdin to write it to that would mean what
    /// the caller asked for, and the restart it really needs would kill the turn
    /// the user is watching.
    ///
    /// The other half of the rule is
    /// [`a_steer_from_idle_restarts_for_the_settings_it_asks_for`]: from idle
    /// the same press *is* the restart.
    #[tokio::test]
    async fn a_message_that_needs_new_settings_cannot_be_steered_into_a_running_turn() {
        let harness = Harness::ordinary().await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send("one");
        until("the turn", || harness.state().phase == Phase::Working).await;
        harness.send_with("under a different model", harness.recipe(a_settings_change()));
        until("it to be queued", || harness.state().queued == 1).await;

        let qid = harness.qid("different model");
        let outcome = harness.steer(&qid).await;
        let QueueOutcome::NeedsRestart { differences } = &outcome else {
            panic!("a turn is in flight, so the process cannot be replaced under it: {outcome:?}");
        };
        assert!(
            differences.contains("opus"),
            "and the refusal names the setting in the way, because the route has nothing else to show: {differences}"
        );
        let queued = harness.queued();
        assert_eq!(queued.len(), 1, "a refused steer leaves the queue exactly as it was");
        assert!(!queued[0].steered, "{queued:?}");
        assert_eq!(harness.seen().len(), 1, "and nothing was written to the process it does not fit");
        assert!(harness.spawns().is_empty(), "nor was anything restarted under the turn");

        // Unknown ids are refused too, and are the ordinary case rather than a
        // typo: a message whose turn started while the request was in flight.
        assert_eq!(harness.steer("no-such-message").await, QueueOutcome::Unknown);
        assert_eq!(harness.withdraw("no-such-message").await, QueueOutcome::Unknown);
    }

    /// A queued message can be taken back, and a steered one cannot: the second
    /// is in claude's stdin, and nothing here can reach into another process'
    /// buffer.
    #[tokio::test]
    async fn a_queued_message_can_be_withdrawn_until_it_is_steered() {
        let harness = Harness::ordinary().await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send("one");
        until("the turn", || harness.state().phase == Phase::Working).await;
        harness.send("regretted");
        harness.send("urgent");
        until("both to be queued", || harness.state().queued == 2).await;

        let regretted = harness.qid("regretted");
        let urgent = harness.qid("urgent");
        assert_eq!(harness.steer(&urgent).await, QueueOutcome::Done);
        assert_eq!(
            harness.withdraw(&urgent).await,
            QueueOutcome::AlreadySteered,
            "claude has it; there is no taking it back"
        );
        assert_eq!(
            harness.steer(&urgent).await,
            QueueOutcome::AlreadySteered,
            "and steering it twice would write it twice"
        );

        assert_eq!(harness.withdraw(&regretted).await, QueueOutcome::Done);
        assert_eq!(harness.state().queued, 1, "only the steered one is left");
        assert_eq!(harness.withdraw(&regretted).await, QueueOutcome::Unknown, "and it is gone for good");

        harness.release();
        until("the steered message to run", || harness.seen().len() == 2).await;
        harness.release();
        until("the session to go quiet", || {
            harness.state().queued == 0 && harness.state().phase == Phase::Idle
        })
        .await;
        let seen = harness.seen();
        assert_eq!(seen.len(), 2, "the withdrawn message never reached claude: {seen:?}");
        assert!(!seen.iter().any(|line| line.contains("regretted")), "{seen:?}");
    }

    /// A steer writes into a pipe, and a pipe whose reader dies was never read.
    ///
    /// The same rule the queue follows and the turn in flight does not: a
    /// message claude never saw is delivered to its replacement, and a turn that
    /// half-ran is failed rather than repeated.
    #[tokio::test]
    async fn a_steered_message_is_delivered_again_when_the_process_dies_holding_it() {
        let harness = Harness::ordinary_with(die_when_released()).await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send("doomed");
        until("the turn", || harness.state().phase == Phase::Working).await;
        harness.send("urgent");
        until("the queue", || harness.state().queued == 1).await;
        let urgent = harness.qid("urgent");
        assert_eq!(harness.steer(&urgent).await, QueueOutcome::Done);
        assert!(harness.queued()[0].steered);

        // The process dies with the steered line unread in its stdin.
        harness.release();
        until("the steered message to reach the replacement", || {
            harness.after().iter().any(|line| line.contains("\"urgent\""))
        })
        .await;
        until("its turn to finish", || {
            harness.state().queued == 0 && harness.injection().is_some_and(|run| run.state == InjectionRunState::Ok)
        })
        .await;
        assert!(
            !harness.after().iter().any(|line| line.contains("\"doomed\"")),
            "the turn that was in flight is still not retried: {:?}",
            harness.after()
        );
    }

    // ---------------------------------------------------------- the watchdog

    /// The lesson this whole watchdog rewrite exists for: a quiet turn is
    /// *reported*, never ended. A long tool call is a legitimate thing to be
    /// doing, and killing it destroys work nobody can get back.
    #[tokio::test]
    async fn a_silent_turn_is_marked_stalled_and_left_running() {
        let harness = Harness::start(
            hold_until_released(),
            Duration::from_millis(60),
            Duration::from_millis(30),
            Duration::from_secs(3600),
        )
        .await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("a long one");
        until("the turn", || harness.state().phase == Phase::Working).await;
        until("the stall mark", || harness.state().stalled).await;

        // Nothing was ended: the turn is still in flight, and finishing it
        // normally still works.
        assert_eq!(harness.state().phase, Phase::Working, "a stalled turn is still a turn");
        assert_eq!(harness.injection().expect("a run").state, InjectionRunState::Running);
        harness.release();
        until("the result", || harness.state().phase == Phase::Idle).await;
        assert_eq!(harness.injection().expect("a run").state, InjectionRunState::Ok);
        assert!(!harness.state().stalled, "and the mark is gone with the turn");
    }

    /// The turn's clock, which a list row reads as "working · 3m": set when a
    /// turn begins, held while it runs, and gone when it ends.
    #[tokio::test]
    async fn a_turn_carries_its_start_time_until_it_ends() {
        let harness = Harness::start(
            hold_until_released(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
        )
        .await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        assert_eq!(harness.state().turn_started_at, None, "between turns there is no clock");
        let before = iso8601(SystemTime::now());
        harness.send("a long one");
        until("the turn", || harness.state().phase == Phase::Working).await;
        let started = harness.state().turn_started_at.expect("a working turn has a start");
        assert!(started >= before, "started when the turn did: {started} vs {before}");
        let info: crate::sessions::ResidentInfo = harness.state().into();
        assert_eq!(info.turn_started_at.0.as_deref(), Some(started.as_str()), "and it is on the wire");
        harness.release();
        until("the result", || harness.state().phase == Phase::Idle).await;
        assert_eq!(harness.state().turn_started_at, None, "and the clock stops with the turn");
    }

    /// An idle resident is not on any clock at all. Under the old watchdog it
    /// was: a process between messages made no progress by definition, so the
    /// twenty-minute rule would have shot every one of them.
    #[tokio::test]
    async fn an_idle_resident_is_never_marked_however_long_it_waits() {
        let harness = Harness::start(
            hold_until_released(),
            Duration::from_millis(20),
            Duration::from_millis(20),
            Duration::from_secs(3600),
        )
        .await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!harness.state().stalled, "an idle process is not a stuck one");
        assert!(!harness.driver.is_finished(), "and it is emphatically still alive");
        // And it still answers.
        harness.send("still here?");
        until("the turn", || harness.state().phase == Phase::Working).await;
        harness.release();
        until("the result", || harness.state().phase == Phase::Idle).await;
    }

    // ------------------------------------------------- background tasks

    /// The history a re-attached driver reads: a task that started and never
    /// finished, and a turn that ended after it.
    ///
    /// A real shim's ring is finite. A `task_notification` written while no
    /// server was attached, or lapped out of the buffer, or belonging to a
    /// process that died mid-task, leaves exactly this shape behind — and there
    /// is no later event that can ever remove the task, because the only one
    /// that removes it is the one that went missing.
    fn history_with_an_orphan_task() -> String {
        [
            r#"printf '%s\n' '{"type":"system","subtype":"hook_started"}'"#.to_string(),
            format!(r#"printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'"#),
            r#"printf '%s\n' '{"type":"system","subtype":"task_started","task_id":"ghost","description":"a job nobody will ever hear the end of","task_type":"local_agent"}'"#.to_string(),
            r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false}'"#.to_string(),
        ]
        .join("\n")
    }

    /// The history that made ghost turns: an idle session, then one `system`
    /// line written long after the turn it followed had ended.
    ///
    /// Copied from the flight recording of session `83193b4a` (2026-09-07): the
    /// last `result` at 06:21:45, then nothing for fifteen minutes, then
    /// `commands_changed` — a hook had edited the command list — and then the
    /// server restarted.
    fn history_ending_in_a_system_line() -> String {
        [
            format!(r#"printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'"#),
            r#"printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}'"#.to_string(),
            r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false}'"#.to_string(),
            r#"printf '%s\n' '{"type":"system","subtype":"commands_changed","commands":[{"name":"rh-notify"}]}'"#.to_string(),
        ]
        .join("\n")
    }

    /// The bug this reconstruction rule exists for: a restart must not invent a
    /// turn out of a line that is not work.
    ///
    /// Session `83193b4a`, 2026-09-07. The turn ended at 06:21:45; at 06:36:35
    /// claude published `commands_changed` while the session sat idle; at
    /// 06:52:16 the server restarted and re-attached. The replay read that last
    /// line as work, decided the history ended mid-turn, and opened a turn for
    /// it — `running` with nothing in it for fifty-five minutes, one queued
    /// message that could never be written, and a turn mark saved back to
    /// `runs.json` so the *next* restart reconstructed the same ghost. Only a
    /// hand-sent interrupt cleared it.
    #[tokio::test]
    async fn a_system_line_after_the_last_result_is_not_a_turn_to_inherit() {
        let harness = Harness::start_attached(
            &history_ending_in_a_system_line(),
            hold_until_released(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
            Witness::None,
        )
        .await;
        until("the replay to settle", || harness.state().phase == Phase::Idle).await;
        assert!(
            harness.injection().is_none(),
            "a session that is between turns has no run: {:?}",
            harness.injection()
        );
        assert_eq!(harness.record().turn_at, None, "and nothing for the next boot to re-read as a turn");

        // The half that hurt: a message sent afterwards must go in rather than
        // park behind a turn that can never end.
        harness.send("are you there");
        until("the message to reach claude", || harness.seen().len() == 1).await;
    }

    /// The net under the reconstruction: whatever opened a turn, one claude's
    /// own transcript says is over does not get to hold the queue for ever.
    ///
    /// The rule needs both halves, and the test gives both: nothing at all on
    /// stdout for the whole stall window, *and* a transcript that ends on the
    /// assistant's answer and has not been touched since. A real turn fails the
    /// first half — a long tool call still writes tool results — and a turn
    /// waiting inside one fails the second, because the transcript ends on the
    /// `tool_use` that has not come back.
    #[tokio::test]
    async fn a_turn_the_transcript_says_ended_is_let_go_of() {
        let harness = Harness::start(
            hold_until_released(),
            Duration::from_millis(60),
            Duration::from_millis(30),
            Duration::from_secs(3600),
        )
        .await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("a message this fake claude will never answer");
        until("the turn", || harness.state().phase == Phase::Working).await;

        // What claude would have left behind had this turn ended before the
        // attach: an answer, written long enough ago to be settled.
        let transcript = harness.transcript();
        std::fs::write(
            &transcript,
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"done\"}]}}\n",
        )
        .expect("transcript");
        long_ago(&transcript);

        until("the turn to be let go of", || harness.state().phase == Phase::Idle).await;
        assert!(harness.injection().is_none(), "the run is withdrawn, not failed: nothing ran");
        assert!(!harness.state().stalled, "and the session is not reported as stuck either");
    }

    /// Backdate a file past [`TRANSCRIPT_SETTLES_AFTER`], as the shell tests do
    /// with `touch -t`.
    fn long_ago(path: &Path) {
        let when = SystemTime::now() - Duration::from_secs(3600);
        let file = std::fs::File::options().write(true).open(path).expect("open to backdate");
        file.set_times(std::fs::FileTimes::new().set_accessed(when).set_modified(when))
            .expect("backdate");
    }

    /// The four background lines claude 2.1.231 really writes, in order.
    ///
    /// Every field is copied from the real flight recording of session
    /// `3c002577` (2026-08-30): a `task_started` with its `tool_use_id`,
    /// `subagent_type` and `task_type`; a snapshot naming two tasks; a thinner
    /// snapshot that has dropped one and describes the other with less; and the
    /// `task_notification` that ends it. Stepped rather than streamed so the
    /// mirror can be read between each one.
    fn describes_its_background_work() -> String {
        format!(
            r#"
dir=$(dirname "$GO")
await() {{ n=0; while [ ! -f "$dir/$1" ] && [ "$n" -lt 400 ]; do [ -d "$dir" ] || exit 97; sleep 0.02; n=$((n + 1)); done; }}
(
  await started
  printf '%s\n' '{{"type":"system","subtype":"task_started","task_id":"a8725c02","tool_use_id":"toolu_01PfPz9","description":"survey the repository","subagent_type":"general-purpose","task_type":"local_agent"}}'
  await snapshot
  printf '%s\n' '{{"type":"system","subtype":"background_tasks_changed","tasks":[{{"task_id":"a8725c02","task_type":"local_agent","description":"survey the repository"}},{{"task_id":"buli8h4uy","task_type":"local_bash","description":"Fetch PR 6-9 titles and bodies"}}]}}'
  await prune
  printf '%s\n' '{{"type":"system","subtype":"background_tasks_changed","tasks":[{{"task_id":"buli8h4uy","task_type":"local_bash"}}]}}'
  await notify
  printf '%s\n' '{{"type":"system","subtype":"task_notification","task_id":"buli8h4uy","status":"completed","output_file":"","summary":"Fetch PR 6-9 titles and bodies"}}'
) &
{}
"#,
            hold_until_released()
        )
    }

    /// The reported gap: the CLI describes every background task it starts, and
    /// this server kept a count.
    ///
    /// A session could say "2 background tasks" and nothing else — not what they
    /// were, not when they began — while 1,530 lines naming them had gone past
    /// on the same stream. Everything asserted here was on the wire all along.
    #[tokio::test]
    async fn background_tasks_carry_what_claude_said_about_them() {
        let harness = Harness::ordinary_with(&describes_its_background_work()).await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        // `task_started` is the richest line, and the one that names the task.
        harness.nudge("started");
        until("the task", || harness.state().background == 1).await;
        let started = harness.state().tasks;
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].id, "a8725c02");
        assert_eq!(started[0].description.as_ref().map(String::as_str), Some("survey the repository"));
        assert_eq!(started[0].task_type.as_ref().map(String::as_str), Some("local_agent"));
        assert_eq!(
            started[0].tool_use_id.as_ref().map(String::as_str),
            Some("toolu_01PfPz9"),
            "the join key onto the transcript, which only this line carries"
        );
        let first_seen = started[0].started_at.clone();
        assert!(
            first_seen.as_ref().is_some_and(|at| at.contains('T')),
            "first seen by this server's clock, in iso8601: {first_seen:?}"
        );

        // The snapshot adds the second one, and does not disturb the first.
        harness.nudge("snapshot");
        until("the second task", || harness.state().background == 2).await;
        let both = harness.state().tasks;
        assert_eq!(
            both.iter().map(|task| task.id.as_str()).collect::<Vec<_>>(),
            ["a8725c02", "buli8h4uy"],
            "oldest first, which is the one that has been running longest"
        );
        assert_eq!(both[0].started_at, first_seen, "a republished snapshot does not restart the clock");
        assert_eq!(both[1].description.as_ref().map(String::as_str), Some("Fetch PR 6-9 titles and bodies"));
        // The snapshot is the thinner line — it names this same task with no
        // `tool_use_id` at all — and the join key survives it. Without this the
        // key would live for the couple of seconds between `task_started` and
        // the first republished list, which is to say it would never be seen.
        assert_eq!(
            both[0].tool_use_id.as_ref().map(String::as_str),
            Some("toolu_01PfPz9"),
            "a thinner snapshot does not erase the join key the richer line carried"
        );
        assert_eq!(
            both[1].tool_use_id.as_ref(),
            None,
            "and a task only ever named by a snapshot simply has none: null, not invented"
        );

        // The mirror is the API's own answer, so the shape a client sees is
        // asserted here rather than assumed from the driver's.
        let info = crate::sessions::ResidentInfo::from(harness.state());
        assert_eq!(info.background, 2, "the count stays authoritative");
        assert_eq!(info.tasks.len(), 2);
        assert_eq!(info.tasks[1].task_type.as_ref().map(String::as_str), Some("local_bash"));
        assert_eq!(
            info.tasks[0].tool_use_id.as_ref().map(String::as_str),
            Some("toolu_01PfPz9"),
            "and it reaches the client, which is the only place the join can happen"
        );

        // A snapshot is the whole live list: what it omits has ended. And what
        // it describes less richly than `task_started` did keeps the better
        // description rather than having it blanked.
        harness.nudge("prune");
        until("the snapshot to prune", || harness.state().background == 1).await;
        let pruned = harness.state().tasks;
        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned[0].id, "buli8h4uy");
        assert_eq!(
            pruned[0].description.as_ref().map(String::as_str),
            Some("Fetch PR 6-9 titles and bodies"),
            "a thinner snapshot does not blank what the richer line said"
        );

        // And a notification is terminal.
        harness.nudge("notify");
        until("the notification", || harness.state().background == 0).await;
        assert!(harness.state().tasks.is_empty(), "the description goes with the task");
    }

    /// A message that needs a different process from the one running.
    fn a_settings_change() -> Fingerprint {
        Fingerprint {
            model: Some("opus".into()),
            ..Default::default()
        }
    }

    /// The bug this settle exists for: an orphan task must not hold the queue.
    ///
    /// Without it the replayed `task_started` is believed for ever,
    /// `minding_background` is permanently true, and the first message asking
    /// for a different model parks behind a task that does not exist. A parked
    /// driver sits at `Phase::Idle`, so Stop refuses it too — the message can
    /// be neither delivered, nor failed, nor cancelled, for the life of the
    /// process.
    #[tokio::test]
    async fn an_orphan_task_from_the_replay_does_not_hold_the_queue_for_ever() {
        let harness = Harness::start_attached(
            &history_with_an_orphan_task(),
            hold_until_released(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
            Witness::None,
        )
        .await;
        // Nothing has written a subagent transcript, so nothing corroborates the
        // task and the replay's account of it is dropped.
        until("the replay to settle", || harness.state().phase == Phase::Idle).await;
        assert!(harness.transcript().exists(), "the probe had a session to look under");
        assert_eq!(harness.state().background, 0, "an uncorroborated task is not believed");
        // Waited for rather than sampled: a recording is written by a task of
        // its own, so it is behind the mirror by however long that task takes to
        // be scheduled — which on a loaded machine is longer than the mirror
        // update this test just saw.
        until("the recording to say so", || staged(&harness.recording(), crate::record::Stage::Settle)).await;

        // The proof it is not merely counted differently: a settings change goes
        // straight through instead of parking.
        harness.send_with("under a different model", harness.recipe(a_settings_change()));
        until("the replacement to take the message", || harness.after().len() == 1).await;
        assert!(!harness.state().awaiting_background, "nothing was ever parked");
    }

    /// The other direction, which is the reason the drop is conditional: real
    /// background work at the moment of an attach has left a fresh subagent
    /// transcript, and that task is believed and waited for.
    ///
    /// And then the ceiling: a park with no way out is its own bug, because a
    /// parked driver is `Idle` and Stop refuses it. The wait is bounded, loud,
    /// and visible on the row for the whole of it.
    #[tokio::test]
    async fn a_corroborated_task_is_waited_for_but_not_for_ever() {
        let harness = Harness::start_attached(
            &history_with_an_orphan_task(),
            hold_until_released(),
            Duration::from_secs(3600),
            // The watchdog is what comes back to a parked driver and asks again,
            // so it has to tick inside the test.
            Duration::from_millis(50),
            Duration::from_millis(120),
            // The witness: a subagent transcript the process wrote as it
            // started the task, which is what real background work leaves.
            Witness::Fresh,
        )
        .await;
        until("the replay to settle", || harness.state().phase == Phase::Idle).await;
        assert_eq!(harness.state().background, 1, "fresh subagent output corroborates the task");
        // Reconstructed from the history, description and all: a task that
        // survives a restart is exactly the one a user most wants named.
        let replayed = harness.state().tasks;
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].id, "ghost");
        assert_eq!(
            replayed[0].description.as_ref().map(String::as_str),
            Some("a job nobody will ever hear the end of")
        );
        assert_eq!(replayed[0].task_type.as_ref().map(String::as_str), Some("local_agent"));

        harness.send_with("under a different model", harness.recipe(a_settings_change()));
        until("the settings change to park", || harness.state().awaiting_background).await;
        assert_eq!(harness.state().queued, 1, "the message is waiting, and the row says so");
        assert!(harness.after().is_empty(), "and nothing was restarted under it yet");

        // `await_background` in the test registry is 400ms, so the ceiling is
        // reached inside the test rather than in half an hour.
        until("the ceiling to release it", || !harness.after().is_empty()).await;
        assert!(!harness.state().awaiting_background, "the park ended when the ceiling did");
        assert_eq!(harness.state().background, 0, "and the replacement inherited no tasks");
    }

    /// The other way out of a park, and the one a user can actually reach: the
    /// steer button.
    ///
    /// The bug it fixes had no way out at all. A settings change parked behind a
    /// long-lived background shell waits up to half an hour; the driver is
    /// `Idle` for the whole of it, so Stop refuses it; and the steer that says
    /// "now" used to answer "that message asks for different settings" and leave
    /// everything exactly as it was. The message could only be abandoned.
    ///
    /// So from idle the steer *is* the restart — background work and all, which
    /// is the trade the confirmation in the UI is there to put to the user.
    #[tokio::test]
    async fn a_steer_from_idle_restarts_for_the_settings_it_asks_for() {
        let harness = Harness::start_attached(
            &history_with_an_orphan_task(),
            hold_until_released(),
            Duration::from_secs(3600),
            // Deliberately no watchdog inside this test's lifetime. The ceiling
            // is the *other* release, and a test about the button must not be
            // able to pass because a timer let the message through instead.
            Duration::from_secs(3600),
            Duration::from_millis(120),
            Witness::Fresh,
        )
        .await;
        until("the replay to settle", || harness.state().phase == Phase::Idle).await;
        assert_eq!(harness.state().background, 1, "there is real background work here to be traded away");

        harness.send_with("under a different model", harness.recipe(a_settings_change()));
        until("the settings change to park", || harness.state().awaiting_background).await;
        assert!(harness.after().is_empty(), "and nothing was restarted for it on its own");

        let qid = harness.qid("different model");
        assert_eq!(
            harness.steer(&qid).await,
            QueueOutcome::Done,
            "the press is taken at its word: from idle there is a process to replace"
        );

        until("the replacement to take the message", || harness.after().len() == 1).await;
        assert!(
            harness.after()[0].contains("different model"),
            "the message that ended the park is the one that ran: {:?}",
            harness.after()
        );
        assert_eq!(harness.spawns().len(), 1, "a process the driver started itself, not a second stdin line");
        assert_eq!(
            harness.recorded_fingerprint().and_then(|fingerprint| fingerprint.model),
            Some("opus".to_string()),
            "and it is the process the message asked for, as the next server would read it"
        );
        assert!(!harness.state().awaiting_background, "the park is over");
        assert_eq!(harness.state().queued, 0, "with nothing left waiting behind it");
        assert_eq!(
            harness.state().background,
            0,
            "the tasks went with the process the steer replaced, which is what the confirmation said would happen"
        );
    }

    /// The live bug: a directory's mtime says nothing about the files in it.
    ///
    /// Claude creates a background agent's transcript when the agent starts and
    /// then streams into it for as long as the agent runs. Writing to a file
    /// that already exists does not touch its directory, so the `subagents/`
    /// mtime freezes at the moment the last agent was *dispatched* while every
    /// agent under it is still working — measured on a live session with two
    /// running agents as a directory 133 seconds stale over transcripts written
    /// that same second, well past the thirty-second window.
    ///
    /// A probe that stats only the directory therefore calls a busy session
    /// quiet and drops a task set that is entirely real, which is the worst
    /// outcome this settle has: the work goes on, and nothing anywhere shows it.
    #[tokio::test]
    async fn a_task_still_streaming_into_an_old_directory_survives_the_settle() {
        let harness = Harness::start_attached(
            &history_with_an_orphan_task(),
            hold_until_released(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
            Witness::StreamingIntoAnOldDirectory,
        )
        .await;
        until("the replay to settle", || harness.state().phase == Phase::Idle).await;
        assert_eq!(
            harness.state().background,
            1,
            "a transcript written now corroborates the task however old the directory holding it is"
        );
        let kept = harness.state().tasks;
        assert_eq!(kept.len(), 1, "and the task is still named for the panel");
        assert_eq!(kept[0].id, "ghost");
    }

    /// The other half of the widened witness: a background *command* writes no
    /// subagent transcript at all.
    ///
    /// A `local_bash` task streams into `<task-id>.output` under the task root
    /// and leaves `subagents/` untouched — there is no agent to have a
    /// transcript. Reading only the transcript directory makes every such task a
    /// ghost the moment its events fall into a replay gap, whatever it is doing.
    #[tokio::test]
    async fn a_background_command_is_corroborated_by_its_output_file() {
        let harness = Harness::start_attached(
            &history_with_an_orphan_task(),
            hold_until_released(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
            Witness::TaskOutputOnly,
        )
        .await;
        until("the replay to settle", || harness.state().phase == Phase::Idle).await;
        assert!(
            !crate::sessions::transcript::subagent_dir(&harness.transcript()).exists(),
            "the point of this witness is that there is no transcript directory to read"
        );
        assert_eq!(harness.state().background, 1, "a task output file written now corroborates the task");
    }

    /// And the drop still happens, which is what keeps the queue safe.
    ///
    /// Reading the entries makes the probe more willing to believe a replayed
    /// set, so the case it must still refuse is the one where the evidence
    /// exists and is simply old: a transcript that stopped being written long
    /// ago is an agent that is gone, and holding a settings change behind it is
    /// the parking bug [`Driver::settle_background`] exists to prevent.
    #[tokio::test]
    async fn a_task_whose_transcript_stopped_long_ago_is_still_dropped() {
        let harness = Harness::start_attached(
            &history_with_an_orphan_task(),
            hold_until_released(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
            Witness::Stale,
        )
        .await;
        until("the replay to settle", || harness.state().phase == Phase::Idle).await;
        assert_eq!(harness.state().background, 0, "an old transcript is not a sign of life");
        assert!(harness.state().tasks.is_empty(), "and nothing is left to name");
        until("the recording to say so", || staged(&harness.recording(), crate::record::Stage::Settle)).await;

        // The proof it is a drop and not merely a different count: a settings
        // change goes straight through instead of parking behind the ghost.
        harness.send_with("under a different model", harness.recipe(a_settings_change()));
        until("the replacement to take the message", || harness.after().len() == 1).await;
        assert!(!harness.state().awaiting_background, "nothing was ever parked");
    }

    /// A restart takes claude's background work with it, whatever the outgoing
    /// process had told this driver.
    ///
    /// A replacement only announces its task list when the list *changes*, so a
    /// set carried across a respawn is never contradicted: it would hold every
    /// later settings change hostage to work that stopped existing.
    #[tokio::test]
    async fn a_replacement_process_inherits_no_background_tasks() {
        let harness = Harness::ordinary().await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        // A task started live — nothing provisional about this one.
        harness.plan_claude(&replacement_claude(SESSION, FORKED));
        harness.send("start something in the background");
        until("the turn", || harness.state().phase == Phase::Working).await;
        harness.release();
        until("the turn to end", || harness.state().phase == Phase::Idle).await;

        harness.send_with("under a different model", harness.recipe(a_settings_change()));
        until("the replacement to take the message", || harness.after().len() == 1).await;
        assert_eq!(harness.state().background, 0);
        assert!(!harness.state().awaiting_background);
    }

    // ---------------------------------------------------------- the ladder

    /// The first rung ends the *turn*, not the process — which is the whole
    /// reason the ladder starts where it does. A Stop that killed claude would
    /// take every background task it had started with it.
    #[tokio::test]
    async fn an_interrupt_ends_the_turn_without_ending_the_process() {
        // A turn that never ends by itself, over a process that keeps reading
        // stdin — which is what makes the polite rung possible at all: the
        // request has to be *read* mid-turn to be answered mid-turn.
        let harness = Harness::start(
            r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
    *control_request*)
       printf '%s\n' '{"type":"control_response","response":{"subtype":"success"}}'
       printf '%s\n' '{"type":"result","subtype":"interrupted","is_error":false}' ;;
    *) : ;;
  esac
done
"#,
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
        )
        .await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("something long");
        until("the turn", || harness.state().phase == Phase::Working).await;

        assert!(harness.registry.interrupt_resident(SESSION), "a working resident has a turn to stop");
        until("the turn to end", || harness.state().phase == Phase::Idle).await;

        let injection = harness.injection().expect("a run");
        assert_eq!(injection.state, InjectionRunState::Failed);
        assert!(injection.interrupted, "and it says it was interrupted");
        assert_eq!(injection.error.0.as_deref(), Some("interrupted from Droppen"));

        // The polite request is what was written, and the process took it.
        let seen = harness.seen();
        assert!(seen[1].contains("control_request"), "{seen:?}");
        assert!(seen[1].contains("\"subtype\":\"interrupt\""), "{seen:?}");

        // And an idle resident has nothing to stop.
        assert!(!harness.registry.interrupt_resident(SESSION));
        assert!(!harness.registry.interrupt_resident("no-such-session"));

        // The process is still there, and still listening — which is the whole
        // point of stopping a turn this way rather than with a signal.
        assert!(!harness.driver.is_finished());
        harness.send("and the session goes on");
        until("the next message to be read", || harness.seen().len() == 3).await;
    }

    /// A CLI that will not answer gets signalled, rung by rung. The script traps
    /// INT and TERM, so only the last rung can end it — which is what makes the
    /// timing below evidence that all three ran.
    #[tokio::test]
    async fn a_wedged_turn_is_escalated_one_signal_at_a_time() {
        let step = Duration::from_millis(150);
        let harness = Harness::start(
            r#"
trap '' INT TERM
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  await_go
done
"#,
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            step,
        )
        .await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("wedge me");
        until("the turn", || harness.state().phase == Phase::Working).await;

        let started = Instant::now();
        assert!(harness.registry.interrupt_resident(SESSION));
        until("the wedged claude to be killed", || harness.driver.is_finished()).await;
        assert!(
            started.elapsed() >= step * 2,
            "SIGKILL is the third rung, not the first: {:?}",
            started.elapsed()
        );

        let injection = harness.injection().expect("a run");
        assert_eq!(injection.state, InjectionRunState::Failed);
        assert!(injection.interrupted);
        assert_eq!(
            injection.error.0.as_deref(),
            Some("interrupted from Droppen"),
            "a process this server killed on purpose is not reported as a crash"
        );
    }

    // --------------------------------------------------- the recovery ladder

    /// A body that answers nothing: it logs the message, waits for the release,
    /// and dies where a result should have been. What every recovery test needs
    /// — a process that is unambiguously *mid-turn* when it goes.
    fn die_when_released() -> &'static str {
        r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  await_go
  rm -f "$GO"
  exit 9
done
"#
    }

    /// The first rung: a death mid-turn is a `--resume`, the lost turn is
    /// reported as lost rather than silently retried, and the session carries
    /// on in the replacement.
    #[tokio::test]
    async fn a_death_mid_turn_is_recovered_by_resume_and_the_turn_owns_up() {
        let harness = Harness::ordinary_with(die_when_released()).await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("doomed");
        until("the turn", || harness.state().phase == Phase::Working).await;
        harness.release();

        until("the resume", || harness.spawns().len() == 1).await;
        let spawned = harness.spawns().remove(0);
        assert!(spawned.contains(&format!("--resume {SESSION}")), "{spawned}");
        assert!(!spawned.contains("--fork-session"), "the first rung is a plain resume: {spawned}");

        // The turn is failed, not retried: its side effects already ran once.
        until("the turn to be owned up to", || {
            harness.injection().is_some_and(|run| run.state == InjectionRunState::Failed)
        })
        .await;
        let error = harness.injection().expect("a run").error.0.expect("a reason");
        assert!(error.contains("claude exited with code 9"), "{error}");
        assert!(error.contains("the session was recovered; this turn was not"), "{error}");

        // And the session is genuinely back: the replacement answers.
        harness.send("after the fact");
        until("the replacement to answer", || {
            harness.injection().is_some_and(|run| run.state == InjectionRunState::Ok)
        })
        .await;
        assert!(
            harness.after().iter().any(|line| line.contains("after the fact")),
            "{:?}",
            harness.after()
        );
        assert!(!harness.driver.is_finished(), "the session outlives the process");
    }

    /// The queue is the half that *is* redelivered: those messages never
    /// reached the process that died, so running them in the replacement runs
    /// them for the first time.
    #[tokio::test]
    async fn messages_queued_when_claude_died_are_delivered_to_its_replacement() {
        let harness = Harness::ordinary_with(die_when_released()).await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("doomed");
        until("the turn", || harness.state().phase == Phase::Working).await;
        harness.send("survivor");
        until("the queue", || harness.state().queued == 1).await;
        harness.release();

        until("the survivor to arrive", || {
            harness.after().iter().any(|line| line.contains("\"survivor\""))
        })
        .await;
        until("its turn to finish", || {
            harness.state().queued == 0 && harness.injection().is_some_and(|run| run.state == InjectionRunState::Ok)
        })
        .await;
        assert!(
            !harness.after().iter().any(|line| line.contains("\"doomed\"")),
            "the in-flight turn is not retried: {:?}",
            harness.after()
        );
    }

    /// The second rung, and the reason it exists: a replacement that dies
    /// *before saying it is listening* makes the resume itself the suspect, so
    /// the ladder forks — and then follows the id the fork answers under.
    #[tokio::test]
    async fn a_replacement_that_dies_before_listening_is_forked_and_followed() {
        let harness = Harness::ordinary_with(die_when_released()).await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        // The claude every recovery from here on finds: plain resumes are
        // refused before init, a fork is answered — under its own id, because
        // that is what `--fork-session` does.
        harness.plan_claude(&format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$SPAWNS"
case "$*" in
  *--fork-session*) : ;;
  *) exit 91 ;;
esac
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{FORKED}"}}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$AFTER"
  case "$line" in
    *control_request*) : ;;
    *) printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}' ;;
  esac
done
"#
        ));
        harness.send("doomed");
        until("the turn", || harness.state().phase == Phase::Working).await;
        harness.release();

        until("the ladder to reach the fork", || harness.spawns().len() == 3).await;
        let spawns = harness.spawns();
        assert!(spawns[0].contains("--resume") && !spawns[0].contains("--fork-session"), "{spawns:?}");
        assert!(spawns[1].contains("--resume") && !spawns[1].contains("--fork-session"), "{spawns:?}");
        assert!(spawns[2].contains("--fork-session"), "{spawns:?}");

        // The registry followed the fork: the session answers under the new id.
        until("the fork to be adopted", || {
            harness.registry.residents.lock().expect("residents").contains_key(FORKED)
        })
        .await;
        harness.send("in the fork");
        until("the forked session to answer", || {
            harness.injection_for(FORKED).is_some_and(|run| run.state == InjectionRunState::Ok)
        })
        .await;
        assert!(harness.after().iter().any(|line| line.contains("in the fork")), "{:?}", harness.after());
    }

    /// The breaker: a session that cannot be brought back stops being retried,
    /// and the error says what was tried rather than how it went missing.
    #[tokio::test]
    async fn a_crash_loop_trips_the_breaker_and_says_what_was_tried() {
        let harness = Harness::ordinary_with(die_when_released()).await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.refuse_to_start();
        harness.send("doomed");
        until("the turn", || harness.state().phase == Phase::Working).await;
        harness.release();

        until("the breaker to trip", || harness.driver.is_finished()).await;
        assert_eq!(harness.spawns().len(), 3, "resume, resume, fork — and no fourth: {:?}", harness.spawns());
        let error = harness.injection().expect("a run").error.0.expect("a reason");
        assert!(error.contains("could not be recovered"), "{error}");
        assert!(error.contains("--resume --fork-session"), "the error names the rungs: {error}");
        assert!(
            !harness.registry.residents.lock().expect("residents").contains_key(SESSION),
            "a session the breaker gave up on is not still claimed"
        );
    }

    // ------------------------------------------------------- the fingerprint

    /// A message the running process cannot take restarts it, and the message
    /// waits for the replacement rather than going to the wrong one.
    ///
    /// The replacement is a real spawn through a real shim over a fake `claude`
    /// on the `PATH` this recipe carries, because the thing worth pinning is
    /// that the whole path works — the polite shutdown of the old process, the
    /// fresh sidecars a new shim needs, the journal entry moving across, and the
    /// message arriving in the replacement rather than the process it outgrew.
    #[tokio::test]
    async fn a_message_that_does_not_fit_the_running_process_gets_a_new_one() {
        let harness = Harness::ordinary().await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        // The `claude` the replacement will find, and the files it reports through.
        let bin = harness._home.path().join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        let after = harness._home.path().join("after.log");
        std::fs::write(
            bin.join("claude"),
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"{SESSION}\"}}'\n\
                 while IFS= read -r line; do\n  printf '%s\\n' \"$line\" >> \"$AFTER\"\n  \
                 printf '%s\\n' '{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}}'\ndone\n"
            ),
        )
        .expect("fake claude");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }

        let mut env = Env::new();
        env.insert("PATH".into(), format!("{}:/bin:/usr/bin", bin.display()));
        env.insert("AFTER".into(), after.to_string_lossy().into_owned());
        harness.send_with(
            "under a different model",
            Recipe {
                args: vec!["--model".into(), "opus".into()],
                env,
                workdir: Some(harness._home.path().to_path_buf()),
                fingerprint: Fingerprint {
                    model: Some("opus".into()),
                    ..Default::default()
                },
            },
        );

        until("the replacement to take the message", || {
            std::fs::read_to_string(&after).is_ok_and(|log| log.contains("under a different model"))
        })
        .await;
        assert!(
            !harness.seen().iter().any(|line| line.contains("under a different model")),
            "the message must never reach the process it did not fit"
        );
        until("the turn to finish on the new process", || harness.state().phase == Phase::Idle).await;
        assert_eq!(harness.injection().expect("a run").state, InjectionRunState::Ok);

        // One record, for the process that is actually running, pointing at the
        // shim a later boot would have to re-attach through.
        let records = harness.registry.journal.read();
        assert_eq!(records.len(), 1, "{records:?}");
        assert!(records[0].resident);
        assert!(records[0].shim.is_some(), "a resident record names its shim");
        assert_ne!(records[0].run_id, "record-under-test", "the retired record went with the old process");

        // The recording followed the process across: the restart is in it, and
        // so is the spawn that answered it — with the settings that caused it
        // and, emphatically, without the environment that carried the token.
        let events = harness.recording();
        assert!(staged(&events, crate::record::Stage::Respawn), "{events:?}");
        let spawn = events
            .iter()
            .find_map(|event| match event {
                crate::record::Event::Spawned { selector, model, cwd, .. } => Some((selector.clone(), model.clone(), cwd.clone())),
                _ => None,
            })
            .expect("the replacement's spawn line");
        assert_eq!(spawn.0, "resume", "a replacement always resumes the transcript it inherits");
        assert_eq!(spawn.1.as_deref(), Some("opus"), "and says what it was started under");
        assert!(
            spawn.2.is_some(),
            "and where it runs, which is what replay derives a project directory from"
        );
    }

    /// The production failure, at the level it happened.
    ///
    /// A resident spawned before the journal recorded fingerprints, still alive
    /// after the server it belonged to was upgraded, with a message waiting
    /// behind it. Every such message used to be answered with "the settings this
    /// message needs were lost when the server restarted" — because a record
    /// that said nothing was read as a process started with no flags at all,
    /// which nothing an account-bound session ever sends can satisfy.
    ///
    /// Unknown means unknown: the server that accepted these messages compared
    /// them against the fingerprint it had, and this one, knowing less, does not
    /// overrule it.
    #[tokio::test]
    async fn a_resident_with_no_recorded_settings_delivers_the_backlog_it_inherited() {
        let waiting = runs::Queued {
            id: "q-1".to_string(),
            text: "still waiting from before the restart".to_string(),
            fingerprint: Fingerprint {
                model: Some("opus".into()),
                account: Some("work".into()),
                ..Default::default()
            },
            steered: false,
            rotation_attempt: 0,
            continuation: None,
        };
        let harness = Harness::start_inherited(None, vec![waiting]).await;

        until("the inherited message to reach claude", || {
            harness.seen().iter().any(|line| line.contains("still waiting from before the restart"))
        })
        .await;
        harness.release();
        until("the turn to end", || harness.state().phase == Phase::Idle).await;
        assert_eq!(
            harness.injection().expect("a run").state,
            InjectionRunState::Ok,
            "the message ran, rather than being failed for settings nobody wrote down"
        );
        // And nothing was taken from it: a message out of the journal has no
        // environment behind it, so it describes no process this driver could
        // start and is no evidence about the one it is holding.
        assert!(
            harness.recorded_fingerprint().is_none(),
            "a message with no recipe leaves the unknown where it was"
        );
    }

    /// A fresh send to a resident whose settings were never recorded takes the
    /// process over rather than replacing it — and says what it is, once.
    ///
    /// Both halves matter. Restarting would kill whatever the session was
    /// minding in the background over a comparison against a fingerprint that
    /// was never written down; and an unknown that stayed unknown would have to
    /// be guessed at again after every restart, which is how one bad record
    /// outlived three upgrades.
    #[tokio::test]
    async fn a_fresh_message_takes_over_a_resident_whose_settings_were_never_recorded() {
        let harness = Harness::start_inherited(None, Vec::new()).await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        let asked = Fingerprint {
            model: Some("opus".into()),
            account: Some("work".into()),
            ..Default::default()
        };
        harness.send_with("first message after the upgrade", harness.recipe(asked.clone()));
        until("the message to reach the process that is already running", || {
            harness.seen().iter().any(|line| line.contains("first message after the upgrade"))
        })
        .await;
        assert!(
            harness.spawns().is_empty(),
            "a process whose settings nobody recorded is not one to throw away: {:?}",
            harness.spawns()
        );
        harness.release();
        until("the turn to end", || harness.state().phase == Phase::Idle).await;

        // Taken once, and written where the *next* server reads it.
        assert_eq!(
            harness.recorded_fingerprint(),
            Some(asked.clone()),
            "the settings of the message that went in heal the record"
        );

        // Which is what puts the check back: the next message that really does
        // need a different process gets one.
        harness.send_with(
            "and now under another model",
            harness.recipe(Fingerprint {
                model: Some("haiku".into()),
                account: Some("work".into()),
                ..Default::default()
            }),
        );
        until("the replacement to take the message", || {
            harness.after().iter().any(|line| line.contains("and now under another model"))
        })
        .await;
        assert!(
            !harness.seen().iter().any(|line| line.contains("and now under another model")),
            "and the process it did not fit never sees it"
        );
    }

    /// A resident answers to both of its model names, and writes the second
    /// one down.
    ///
    /// The failure that produced the report this whole change came from. A
    /// spawn asks for `--model fable`; `system:init` answers `claude-fable-5`;
    /// the driver replaced the name it asked for with the name it was given,
    /// and every send still saying `fable` then read as a settings change. The
    /// recording shows what that cost: eight turns, eight restarts, the
    /// session's background work killed each time, and the message queued
    /// behind one of them failed with "the settings this message needs were
    /// lost when the server restarted" — because a message out of the journal
    /// has no environment to start a process of its own with.
    ///
    /// Both names, then, and both on the record: the next server inherits the
    /// pair rather than re-learning the second one at the price of a restart.
    #[tokio::test]
    async fn a_resident_answers_to_both_its_model_names_and_records_the_second() {
        let history = format!(r#"printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}","model":"claude-fable-5"}}'"#);
        let asked = Fingerprint {
            model: Some("fable".into()),
            account: Some("work".into()),
            ..Default::default()
        };
        let harness = Harness::build(
            &history,
            hold_until_released(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
            NEVER_SETTLES,
            Adoption {
                // An attach, so the history — and the `init` in it — is replayed
                // before anything is pumped: that ordering is the scenario.
                attach: Some(Witness::None),
                live: Some(asked.clone()),
                queued: vec![runs::Queued {
                    id: "q-1".to_string(),
                    text: "queued under the name the composer sends".to_string(),
                    fingerprint: asked.clone(),
                    steered: false,
                    rotation_attempt: 0,
                    continuation: None,
                }],
                recipe: None,
                hold: None,
                ring: None,
                transcript: None,
                ..Adoption::default()
            },
        )
        .await;

        // The inherited message goes in rather than being failed for a name the
        // CLI chose after it was queued.
        until("the inherited message to reach claude", || {
            harness
                .seen()
                .iter()
                .any(|line| line.contains("queued under the name the composer sends"))
        })
        .await;
        harness.release();
        until("the turn to end", || harness.state().phase == Phase::Idle).await;
        assert_eq!(harness.injection().expect("a run").state, InjectionRunState::Ok);

        // Both names are on the record, so a restart from here compares against
        // either one.
        assert_eq!(
            harness.recorded_fingerprint(),
            Some(Fingerprint {
                resolved: Some("claude-fable-5".into()),
                ..asked.clone()
            }),
            "the name the CLI answered under joins the one the spawn asked for"
        );

        // And a send naming the *other* one is the same process too: no restart,
        // and nothing of what the session was doing is thrown away for it.
        harness.send_with(
            "and now under the name the CLI answers with",
            harness.recipe(Fingerprint {
                model: Some("claude-fable-5".into()),
                ..asked
            }),
        );
        until("the second message to reach the same process", || {
            harness
                .seen()
                .iter()
                .any(|line| line.contains("and now under the name the CLI answers with"))
        })
        .await;
        assert!(harness.spawns().is_empty(), "neither name is a settings change: {:?}", harness.spawns());
    }

    /// The safety net, at the one place a message out of the journal can still
    /// meet a process it does not fit.
    ///
    /// The process this driver inherited dies, and the replacement it starts is
    /// built from the only recipe it has — which came from a message that named
    /// nothing in particular. That is a *different* process from the one the
    /// backlog was queued against, and the message left in the queue has no
    /// environment of its own to start the one it wants. Delivering it anyway
    /// would run somebody's prompt under a model they did not choose and bill it
    /// to an account they did not pick, so it is failed instead — and told
    /// exactly which settings are in the way, because "send it again" is a
    /// perfectly good answer and "the settings were lost" is not.
    #[tokio::test]
    async fn a_message_out_of_the_journal_is_failed_rather_than_run_by_the_wrong_process() {
        let wanted = Fingerprint {
            model: Some("opus".into()),
            account: Some("work".into()),
            ..Default::default()
        };
        let queued = |id: &str, text: &str| runs::Queued {
            id: id.to_string(),
            text: text.to_string(),
            fingerprint: wanted.clone(),
            steered: false,
            rotation_attempt: 0,
            continuation: None,
        };
        let harness = Harness::build(
            &preamble(),
            die_when_released(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
            NEVER_SETTLES,
            Adoption {
                attach: None,
                live: Some(wanted.clone()),
                queued: vec![queued("q-1", "the one in flight"), queued("q-2", "the one behind it")],
                recipe: None,
                hold: None,
                ring: None,
                transcript: None,
                ..Adoption::default()
            },
        )
        .await;

        until("the first message to be taken", || harness.state().phase == Phase::Working).await;
        // The process dies holding it, and the recovery ladder starts a
        // replacement from the recipe this driver has: the harness' own, which
        // names no model and no account.
        harness.release();

        until("the second message to be answered", || {
            harness.injection().is_some_and(|run| run.state == InjectionRunState::Failed) && harness.state().queued == 0
        })
        .await;
        let error = harness.injection().expect("a run").error.0.expect("a reason");
        assert!(
            error.contains("model opus rather than none") && error.contains("the work account rather than the local login"),
            "the failure names every setting in the way: {error}"
        );
        assert!(error.contains("send it again"), "and what to do about it: {error}");
        assert!(
            !harness.after().iter().any(|line| line.contains("the one behind it")),
            "and the replacement it did not fit never sees it: {:?}",
            harness.after()
        );
    }

    // ------------------------------------------------------ account rotation

    /// The `result` a refused turn ends with, as claude 2.1.266 writes it — the
    /// apostrophe in "You've" dropped, because the line has to fit inside a
    /// single-quoted shell string.
    const SESSION_LIMIT: &str =
        r#"{"type":"result","subtype":"success","is_error":true,"result":"You have hit your session limit","api_error_status":429}"#;

    /// The Fable weekly refusal, word for word but for the same apostrophe.
    const FABLE_LIMIT: &str = r#"{"type":"result","subtype":"success","is_error":true,"result":"You have reached your Fable limit. Switch to another model, or manage usage credits at claude.ai/settings/usage?from=cc_cli_limit_message, to continue.","api_error_status":429}"#;

    /// An ordinary finished turn.
    const TURN_OK: &str = r#"{"type":"result","subtype":"success","is_error":false}"#;

    /// A claude refused on its first message — the refused `rate_limit_event`
    /// for `window` and then `result`, the order every recording shows — that
    /// answers every later message normally. `before` is written first, for a
    /// background task the refusal will find running. Every turn waits for
    /// `$GO`, so a test can queue messages behind it. The counter is not `n`:
    /// `await_go` is a shell function, and shell functions share their
    /// caller's variables.
    fn refused_once(window: &str, resets_at: i64, result: &str, before: &str) -> String {
        format!(
            r#"
turns=0
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in *control_request*) continue ;; esac
  turns=$((turns + 1))
  await_go
  rm -f "$GO"
  if [ "$turns" = 1 ]; then
{before}
    printf '%s\n' '{{"type":"rate_limit_event","rate_limit_info":{{"status":"rejected","rateLimitType":"{window}","resetsAt":{resets_at},"overageStatus":"rejected","isUsingOverage":false}}}}'
    printf '%s\n' '{result}'
  else
    printf '%s\n' '{TURN_OK}'
  fi
done
"#
        )
    }

    /// The `claude` a rotation's respawn finds: it notes its argv and which
    /// token it was given — a fake one, minted by the test — and answers every
    /// message with `answer`.
    fn rotated_claude(answer: &str) -> String {
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$SPAWNS"
printf 'token=%s\n' "${{CLAUDE_CODE_OAUTH_TOKEN:-none}}" >> "$SPAWNS"
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$AFTER"
  case "$line" in
    *control_request*) : ;;
    *) printf '%s\n' '{answer}' ;;
  esac
done
"#
        )
    }

    /// One background task, announced the way claude announces them.
    fn one_background_task(id: &str, description: &str) -> String {
        format!(
            r#"    printf '%s\n' '{{"type":"system","subtype":"background_tasks_changed","tasks":[{{"task_id":"{id}","task_type":"local_bash","description":"{description}"}}]}}'"#
        )
    }

    /// The fake token a test account holds. Not a credential of anybody's.
    fn fake_token(label: &str) -> String {
        format!("sk-ant-oat01-test-{}", label.replace(['@', ' '], "-"))
    }

    /// Store `wanted` accounts in `home` — `(label, oauth)`: an OAuth account
    /// is in the pool by default, an env one is not — bind the session to the
    /// first, and describe a process started under it running `model`.
    async fn accounts_in(home: &Path, wanted: &[(&str, bool)], model: Option<&str>) -> (Vec<crate::accounts::store::Account>, Fingerprint) {
        use crate::accounts::store::{parse_env_block, AccountsStore, Credential};
        let store = AccountsStore::new(Arc::new(Paths::new(home.to_path_buf())));
        let mut accounts = Vec::new();
        for (label, oauth) in wanted {
            let credential = if *oauth {
                Credential::Oauth(fake_token(label))
            } else {
                Credential::Env(
                    parse_env_block(&format!(
                        "ANTHROPIC_AUTH_TOKEN={}\nANTHROPIC_BASE_URL=https://relay.example",
                        fake_token(label)
                    ))
                    .expect("block"),
                )
            };
            accounts.push(store.create(label, AccountProvider::Claude, credential).await.expect("account"));
        }
        let first = &accounts[0];
        store
            .bind_session_account(SESSION, Some(&first.id), AccountProvider::Claude)
            .await
            .expect("bind");
        let live = Fingerprint {
            model: model.map(str::to_owned),
            account: Some(first.id.clone()),
            credential: store.credential_hash(&first.id).await,
            ..Default::default()
        };
        (accounts, live)
    }

    impl Harness {
        /// A driver whose process runs under an account, in a home that
        /// already holds the accounts.
        async fn rotating(home: tempfile::TempDir, body: &str, live: Fingerprint) -> Harness {
            Harness::build_in(
                home,
                &preamble(),
                body,
                Duration::from_secs(3600),
                Duration::from_secs(3600),
                Duration::from_millis(120),
                NEVER_SETTLES,
                Adoption {
                    attach: None,
                    live: Some(live.clone()),
                    queued: Vec::new(),
                    recipe: Some(live),
                    hold: None,
                    ring: None,
                    transcript: None,
                    ..Adoption::default()
                },
            )
            .await
        }

        /// The argv lines of the processes a rotation started.
        fn respawns(&self) -> Vec<String> {
            self.spawns().into_iter().filter(|line| !line.starts_with("token=")).collect()
        }

        /// The token each of those processes was handed.
        fn tokens(&self) -> Vec<String> {
            self.spawns()
                .into_iter()
                .filter_map(|line| line.strip_prefix("token=").map(str::to_owned))
                .collect()
        }

        fn latest_rotation(&self) -> Option<rotation::SessionRotation> {
            self.registry.accounts.rotation_log().latest(SESSION)
        }
    }

    /// The whole of phase 2, on the everyday case: the five-hour window runs
    /// out mid-conversation with a message queued behind the turn.
    ///
    /// The queued message must not go to the account that just refused — that
    /// is the bug, five rejections in a minute in one real session — and the
    /// failed message must not be sent again: it is in the transcript already.
    /// What goes to the next account is a continuation, then the queue.
    #[tokio::test]
    async fn a_session_limit_moves_the_session_to_the_next_pool_account_and_carries_on() {
        let home = tempfile::tempdir().expect("tempdir");
        let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], None).await;
        let (a, b) = (accounts[0].clone(), accounts[1].clone());
        let resets = rest::now_ms() / 1000 + 7200;
        let harness = Harness::rotating(home, &refused_once("five_hour", resets, SESSION_LIMIT, ""), live.clone()).await;
        harness.plan_claude(&rotated_claude(TURN_OK));
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send_with("first", harness.recipe(live.clone()));
        until("the first turn", || harness.state().phase == Phase::Working).await;
        harness.send_with("second", harness.recipe(live.clone()));
        until("the second message to queue", || harness.state().queued == 1).await;
        harness.release();

        until("the continuation and the queued message to reach the new account", || {
            harness.after().len() == 2 && harness.state().phase == Phase::Idle
        })
        .await;
        let seen = harness.seen();
        assert_eq!(seen.len(), 1, "the refusing account got the first message and nothing else: {seen:?}");
        assert!(seen[0].contains("first"));
        let after = harness.after();
        assert!(
            after[0].contains("[Droppen] 这个会话已从账号 a@work 自动切换到 b@team：a@work 的 5 小时额度用完"),
            "the new account is told what happened instead of being sent the prompt again: {}",
            after[0]
        );
        assert!(after[0].contains("请从中断处继续"), "{}", after[0]);
        assert!(
            !after.iter().any(|line| line.contains("\"first\"")),
            "the failed message is not re-sent: {after:?}"
        );
        assert!(after[1].contains("second"), "and the queue follows it there: {after:?}");

        assert_eq!(harness.respawns().len(), 1, "one respawn: {:?}", harness.spawns());
        assert!(
            harness.respawns()[0].contains(&format!("--resume {SESSION}")),
            "the same conversation, resumed"
        );
        assert_eq!(harness.tokens(), vec![fake_token("b@team")], "under the new account's token");
        assert_eq!(
            harness.registry.accounts.session_account_id(SESSION).await.as_deref(),
            Some(b.id.as_str()),
            "the binding moved with it"
        );
        assert_eq!(harness.recorded_fingerprint().and_then(|live| live.account), Some(b.id.clone()));

        let rotation = harness.latest_rotation().expect("the decision is on record");
        assert_eq!(rotation.outcome, RotationOutcome::Switched);
        assert_eq!((rotation.from.id.as_str(), rotation.to.id.as_str()), (a.id.as_str(), b.id.as_str()));
        assert_eq!(rotation.from.label, "a@work");
        assert_eq!(rotation.class, LimitClass::Session);
        assert_eq!(rotation.scope, "all");
        assert_eq!(
            rotation.until_at.0,
            Some(rest::iso_ms(resets * 1000)),
            "the reset the refused event named"
        );
        assert_eq!(rotation.interrupted_tasks, 0);
        assert!(
            staged(&harness.recording(), crate::record::Stage::Rotate),
            "and the flight recording says so"
        );
    }

    /// Nowhere to go: the only other account is outside the pool. The queue is
    /// held — not fed to the refusing account, and not dropped — until somebody
    /// sends another message, and then it goes in order.
    #[tokio::test]
    async fn a_limit_with_nowhere_to_go_holds_the_queue_instead_of_feeding_the_exhausted_account() {
        let home = tempfile::tempdir().expect("tempdir");
        let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("relay", false)], None).await;
        let resets = rest::now_ms() / 1000 + 7200;
        let harness = Harness::rotating(home, &refused_once("five_hour", resets, SESSION_LIMIT, ""), live.clone()).await;
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send_with("first", harness.recipe(live.clone()));
        until("the first turn", || harness.state().phase == Phase::Working).await;
        harness.send_with("second", harness.recipe(live.clone()));
        until("the second message to queue", || harness.state().queued == 1).await;
        harness.release();

        until("the decision", || harness.latest_rotation().is_some()).await;
        let rotation = harness.latest_rotation().expect("a decision");
        assert_eq!(rotation.outcome, RotationOutcome::NoCandidate, "an env account is not in the pool");
        assert_eq!(
            rotation.to.id, accounts[0].id,
            "a decision that moved nothing names the account it stayed on"
        );
        assert_eq!(
            harness.injection().map(|run| run.state),
            Some(InjectionRunState::Failed),
            "the refused turn stays failed"
        );

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(harness.seen().len(), 1, "nothing more went to the refusing account: {:?}", harness.seen());
        assert_eq!(harness.state().queued, 1, "and nothing was dropped");
        assert_eq!(harness.state().phase, Phase::Idle);
        assert!(harness.spawns().is_empty(), "and nothing was respawned");
        let journalled = harness.record().hold.expect("the hold is on the record, for the next server");
        assert_eq!(journalled.account, accounts[0].id);
        assert!(journalled.continue_failed);

        // Somebody acts: the held message goes first, then the new one.
        harness.send_with("third", harness.recipe(live.clone()));
        until("the held message to go in", || harness.seen().len() == 2).await;
        assert_eq!(harness.record().hold, None, "and the record stops saying so");
        assert!(harness.seen()[1].contains("second"), "{:?}", harness.seen());
        harness.release();
        until("the new one after it", || harness.seen().len() == 3).await;
        assert!(harness.seen()[2].contains("third"), "{:?}", harness.seen());
        harness.release();
        until("the queue to drain", || {
            harness.state().queued == 0 && harness.state().phase == Phase::Idle
        })
        .await;
    }

    /// Three switches for one message, and then no more: every account the
    /// chain reaches refuses too, and the fourth refusal leaves the run failed
    /// rather than bouncing on.
    #[tokio::test]
    async fn the_attempt_cap_stops_a_message_after_three_switches() {
        let home = tempfile::tempdir().expect("tempdir");
        let (accounts, live) = accounts_in(home.path(), &[("a", true), ("b", true), ("c", true), ("d", true)], None).await;
        let resets = rest::now_ms() / 1000 + 7200;
        let harness = Harness::rotating(home, &refused_once("five_hour", resets, SESSION_LIMIT, ""), live.clone()).await;
        harness.plan_claude(&rotated_claude(SESSION_LIMIT));
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send_with("first", harness.recipe(live.clone()));
        harness.release();
        until("the cap", || {
            harness
                .latest_rotation()
                .is_some_and(|rotation| rotation.outcome == RotationOutcome::CapReached)
        })
        .await;

        let history = harness.registry.accounts.rotation_log().history(SESSION, 10);
        let steps: Vec<(RotationOutcome, &str, &str)> = history
            .iter()
            .rev()
            .map(|rotation| (rotation.outcome, rotation.from.label.as_str(), rotation.to.label.as_str()))
            .collect();
        assert_eq!(
            steps,
            vec![
                (RotationOutcome::Switched, "a", "b"),
                (RotationOutcome::Switched, "b", "c"),
                (RotationOutcome::Switched, "c", "d"),
                (RotationOutcome::CapReached, "d", "d"),
            ],
            "each refusal moves on to an account that is not resting, three times"
        );
        assert_eq!(harness.respawns().len(), 3, "{:?}", harness.spawns());
        assert_eq!(harness.tokens(), vec![fake_token("b"), fake_token("c"), fake_token("d")]);
        assert_eq!(harness.after().len(), 3, "one continuation each: {:?}", harness.after());
        assert_eq!(harness.injection().map(|run| run.state), Some(InjectionRunState::Failed));
        assert_eq!(
            harness.registry.accounts.session_account_id(SESSION).await.as_deref(),
            Some(accounts[3].id.as_str())
        );
    }

    /// A limit on the whole account does not wait for background work — it is
    /// billing the same exhausted credential — and the continuation names what
    /// the restart cut short, with the task's output file.
    #[tokio::test]
    async fn a_whole_account_limit_switches_at_once_and_lists_the_background_work_it_cut() {
        let home = tempfile::tempdir().expect("tempdir");
        let (_, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], None).await;
        let tasks = home.path().join("task-root").join("a-project").join(SESSION).join("tasks");
        std::fs::create_dir_all(&tasks).expect("tasks dir");
        std::fs::write(tasks.join("bt1.output"), "running\n").expect("output");
        let resets = rest::now_ms() / 1000 + 7200;
        let body = refused_once("five_hour", resets, SESSION_LIMIT, &one_background_task("bt1", "跑完整测试"));
        let harness = Harness::rotating(home, &body, live.clone()).await;
        harness.plan_claude(&rotated_claude(TURN_OK));
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send_with("first", harness.recipe(live.clone()));
        harness.release();
        until("the continuation", || harness.after().len() == 1).await;

        let continuation = &harness.after()[0];
        assert!(
            continuation.contains("以下后台子任务被中断，请检查它们的输出后按需重新派发："),
            "{continuation}"
        );
        assert!(continuation.contains("- 跑完整测试（输出："), "{continuation}");
        assert!(continuation.contains("bt1.output"), "{continuation}");
        let rotation = harness.latest_rotation().expect("a decision");
        assert_eq!(rotation.interrupted_tasks, 1);
        assert_eq!(harness.respawns().len(), 1);
        until("the replacement to settle", || harness.state().phase == Phase::Idle).await;
        assert_eq!(harness.state().background, 0, "the tasks went with the old process");
    }

    /// A limit on one model family lets claude's background work finish first
    /// — a subagent on another model can still complete — and only then moves
    /// the session.
    #[tokio::test]
    async fn a_family_limit_waits_for_background_work_before_switching() {
        let home = tempfile::tempdir().expect("tempdir");
        let (_, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], Some("fable")).await;
        let resets = rest::now_ms() / 1000 + 3 * 86_400;
        let finishes = r#"
dir=$(dirname "$GO")
(
  n=0
  while [ ! -f "$dir/tasks-done" ] && [ "$n" -lt 400 ]; do [ -d "$dir" ] || exit 97; sleep 0.02; n=$((n + 1)); done
  [ -f "$dir/tasks-done" ] && printf '%s\n' '{"type":"system","subtype":"background_tasks_changed","tasks":[]}'
) &
"#;
        let body = format!(
            "{finishes}{}",
            refused_once(
                "seven_day_overage_included",
                resets,
                FABLE_LIMIT,
                &one_background_task("bt1", "Review the diff")
            )
        );
        let harness = Harness::rotating(home, &body, live.clone()).await;
        harness.plan_claude(&rotated_claude(TURN_OK));
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send_with("first", harness.recipe(live.clone()));
        harness.release();
        until("the decision", || harness.latest_rotation().is_some()).await;
        let rotation = harness.latest_rotation().expect("a decision");
        assert_eq!((rotation.class, rotation.scope.as_str()), (LimitClass::Weekly, "fable"));
        assert_eq!(rotation.until_at.0, Some(rest::iso_ms(resets * 1000)), "the reset the event named");
        until("the switch to park behind the background work", || harness.state().awaiting_background).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(harness.spawns().is_empty(), "no restart while the task runs: {:?}", harness.spawns());
        assert!(harness.after().is_empty());

        harness.nudge("tasks-done");
        until("the continuation, once the task is done", || harness.after().len() == 1).await;
        let continuation = &harness.after()[0];
        assert!(continuation.contains("a@work 的 Fable 每周额度用完"), "{continuation}");
        assert!(!continuation.contains("以下后台子任务"), "nothing was cut short: {continuation}");
        assert_eq!(harness.latest_rotation().expect("a decision").interrupted_tasks, 0);
        assert_eq!(harness.tokens(), vec![fake_token("b@team")]);
    }

    /// Rest `account` under `scope` until `until_ms`, as phase 1 would have.
    fn rest_on(registry: &InjectionRegistry, account: &str, class: LimitClass, scope: &str, until_ms: i64) {
        registry
            .accounts
            .limit_log()
            .record(
                &Observation {
                    account_id: account.to_string(),
                    session_id: Some("elsewhere".into()),
                    source: "claude:result",
                    model: None,
                    verdict: Verdict {
                        class,
                        scope: scope.to_string(),
                        until_ms: Some(until_ms),
                    },
                    raw: "You have hit your session limit".into(),
                    reason: "You have hit your session limit".into(),
                    may_rest: true,
                },
                rest::now_ms(),
            )
            .expect("rest");
    }

    /// A message as the journal keeps it.
    fn journalled(id: &str, text: &str, fingerprint: &Fingerprint, rotation_attempt: u8, continuation: Option<Continuation>) -> runs::Queued {
        runs::Queued {
            id: id.to_string(),
            text: text.to_string(),
            fingerprint: fingerprint.clone(),
            steered: false,
            rotation_attempt,
            continuation,
        }
    }

    /// Backdate a directory or a file the fake claude wrote, so nothing about it
    /// reads as background work still streaming.
    fn quiet(path: &Path) {
        let when = SystemTime::now() - Duration::from_secs(3600);
        let file = std::fs::File::open(path).expect("open to backdate");
        file.set_times(std::fs::FileTimes::new().set_accessed(when).set_modified(when))
            .expect("backdate");
    }

    impl Harness {
        /// [`Harness::rotating`] with a watchdog that ticks inside the test, a
        /// backlog and a hold the journal record already holds.
        async fn rotating_with(
            home: tempfile::TempDir,
            body: &str,
            live: Fingerprint,
            tick: Duration,
            queued: Vec<runs::Queued>,
            hold: Option<Hold>,
        ) -> Harness {
            Harness::build_in(
                home,
                &preamble(),
                body,
                Duration::from_secs(3600),
                tick,
                Duration::from_millis(120),
                NEVER_SETTLES,
                Adoption {
                    attach: None,
                    live: Some(live.clone()),
                    queued,
                    recipe: Some(live),
                    hold,
                    ring: None,
                    transcript: None,
                    ..Adoption::default()
                },
            )
            .await
        }

        fn rotations(&self) -> Vec<rotation::SessionRotation> {
            self.registry.accounts.rotation_log().history(SESSION, 20)
        }
    }

    /// M2: a server restarted while its queue was held keeps holding it. The
    /// pump used to feed the queue to the account that had refused the moment
    /// the new server attached, fail it, and decide — and notify — all over
    /// again. The watchdog's re-checks while the account still rests decide
    /// nothing either.
    #[tokio::test]
    async fn a_restart_during_a_hold_keeps_the_queue_held() {
        let home = tempfile::tempdir().expect("tempdir");
        let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("relay", false)], None).await;
        let a = accounts[0].clone();
        let lifts = rest::now_ms() + 2 * 3600 * 1000;
        let hold = Hold {
            account: a.id.clone(),
            label: a.label.clone(),
            class: LimitClass::Session,
            scope: "all".into(),
            until_ms: Some(lifts),
            switches: 0,
            continue_failed: true,
        };
        let queued = vec![journalled("q-1", "second", &live, 0, None)];
        let harness = Harness::rotating_with(
            home,
            hold_until_released(),
            live.clone(),
            Duration::from_millis(50),
            queued,
            Some(hold.clone()),
        )
        .await;
        rest_on(&harness.registry, &a.id, LimitClass::Session, "all", lifts);
        until("the init", || harness.state().phase == Phase::Idle).await;

        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            harness.seen().is_empty(),
            "nothing went to the account that refused: {:?}",
            harness.seen()
        );
        assert_eq!(harness.state().queued, 1, "and nothing was dropped");
        assert!(
            harness.rotations().is_empty(),
            "no second decision about the same limit, however often it was re-checked"
        );
        assert_eq!(harness.record().hold, Some(hold), "and the record still says it is held");
    }

    /// M1: a server restarted while a continuation waited behind background
    /// work. The continuation and the message retargeted behind it name the new
    /// account, the running process is still on the old one, and the journal
    /// cannot carry an environment — so they used to be dropped at adoption.
    /// Now they are kept, rebuilt for the account the session is bound to, and
    /// delivered there once the background work is done.
    #[tokio::test]
    async fn a_restart_during_a_family_wait_keeps_the_continuation_and_moves_to_the_new_account() {
        let home = tempfile::tempdir().expect("tempdir");
        let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], Some("fable")).await;
        let b = accounts[1].clone();
        let store = crate::accounts::store::AccountsStore::new(Arc::new(Paths::new(home.path().to_path_buf())));
        store
            .bind_session_account(SESSION, Some(&b.id), AccountProvider::Claude)
            .await
            .expect("bind");
        let under_b = Fingerprint {
            account: Some(b.id.clone()),
            credential: store.credential_hash(&b.id).await,
            ..live.clone()
        };
        let why = rotation::reason(LimitClass::Weekly, "fable");
        let note = Continuation {
            record: None,
            from: "a@work".into(),
            to: "b@team".into(),
            reason: why.clone(),
            until: None,
        };
        let queued = vec![
            journalled(
                "c-1",
                &rotation::continuation_prompt("a@work", "b@team", &why, None, &[]),
                &under_b,
                1,
                Some(note),
            ),
            journalled("q-2", "second", &under_b, 0, None),
        ];
        let finishes = r#"
dir=$(dirname "$GO")
(
  n=0
  while [ ! -f "$dir/tasks-done" ] && [ "$n" -lt 400 ]; do [ -d "$dir" ] || exit 97; sleep 0.02; n=$((n + 1)); done
  [ -f "$dir/tasks-done" ] && printf '%s\n' '{"type":"system","subtype":"background_tasks_changed","tasks":[]}'
) &
"#;
        let harness = Harness::build_in(
            home,
            &history_with_an_orphan_task(),
            &format!("{finishes}{}", hold_until_released()),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
            NEVER_SETTLES,
            Adoption {
                attach: Some(Witness::Fresh),
                live: Some(live.clone()),
                queued,
                recipe: Some(live.clone()),
                hold: None,
                ring: None,
                transcript: None,
                ..Adoption::default()
            },
        )
        .await;
        harness.plan_claude(&rotated_claude(TURN_OK));
        until("the continuation to park behind the background work", || {
            harness.state().awaiting_background
        })
        .await;
        assert_eq!(harness.state().queued, 2, "both survived the restart: {:?}", harness.queued());
        assert!(harness.queued()[0].text.contains("自动切换到 b@team"));
        assert!(harness.spawns().is_empty(), "and nothing was restarted while the task ran");

        // The task ends: its transcript goes quiet and claude says so.
        let subagents = crate::sessions::transcript::subagent_dir(&harness.transcript());
        quiet(&subagents.join("agent-ghost.jsonl"));
        quiet(&subagents);
        harness.nudge("tasks-done");
        until("the continuation and the message behind it to reach the new account", || {
            harness.after().len() == 2
        })
        .await;
        let after = harness.after();
        assert!(after[0].contains("a@work 的 Fable 每周额度用完"), "{}", after[0]);
        assert!(after[1].contains("second"), "{after:?}");
        assert!(harness.seen().is_empty(), "the old account got neither: {:?}", harness.seen());
        assert_eq!(harness.tokens(), vec![fake_token("b@team")], "under the account the session is bound to");
        assert!(
            harness.rotations().is_empty(),
            "the decision was the last server's; this one only carried it out"
        );
    }

    /// A claude that is refused once, holds, and then — on a nudge — runs a
    /// turn of its own (a background task finishing, say) that the same
    /// account refuses again.
    fn refused_twice(resets_at: i64) -> String {
        format!(
            r#"
dir=$(dirname "$GO")
(
  n=0
  while [ ! -f "$dir/again" ] && [ "$n" -lt 400 ]; do [ -d "$dir" ] || exit 97; sleep 0.02; n=$((n + 1)); done
  if [ -f "$dir/again" ]; then
    printf '%s\n' '{{"type":"assistant","message":{{}}}}'
    printf '%s\n' '{{"type":"rate_limit_event","rate_limit_info":{{"status":"rejected","rateLimitType":"five_hour","resetsAt":{resets_at}}}}}'
    printf '%s\n' '{SESSION_LIMIT}'
  fi
) &
{}
"#,
            refused_once("five_hour", resets_at, SESSION_LIMIT, "")
        )
    }

    /// L1: while the queue is held against an account, another refusal from it
    /// is the same limit. No second decision, no second notification.
    #[tokio::test]
    async fn a_second_refusal_during_a_hold_decides_nothing_new() {
        let home = tempfile::tempdir().expect("tempdir");
        let (_, live) = accounts_in(home.path(), &[("a@work", true), ("relay", false)], None).await;
        let resets = rest::now_ms() / 1000 + 7200;
        let harness = Harness::rotating(home, &refused_twice(resets), live.clone()).await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send_with("first", harness.recipe(live.clone()));
        until("the first turn", || harness.state().phase == Phase::Working).await;
        harness.send_with("second", harness.recipe(live.clone()));
        harness.release();
        until("the hold", || harness.rotations().len() == 1).await;

        harness.nudge("again");
        until("claude's own turn to be refused too", || {
            harness.state().phase == Phase::Idle && harness.injection().is_some_and(|run| run.state == InjectionRunState::Failed)
        })
        .await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(harness.rotations().len(), 1, "one decision about one limit: {:?}", harness.rotations());
        assert_eq!(harness.state().queued, 1, "and the queue is still held");
        assert_eq!(harness.seen().len(), 1, "{:?}", harness.seen());
    }

    /// L1: a held queue asks again every tick, and moves the moment an account
    /// frees up — here, the only other pool account's rest being cleared.
    #[tokio::test]
    async fn a_held_queue_moves_as_soon_as_an_account_frees_up() {
        let home = tempfile::tempdir().expect("tempdir");
        let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], None).await;
        let b = accounts[1].clone();
        let resets = rest::now_ms() / 1000 + 7200;
        let harness = Harness::rotating_with(
            home,
            &refused_once("five_hour", resets, SESSION_LIMIT, ""),
            live.clone(),
            Duration::from_millis(50),
            Vec::new(),
            None,
        )
        .await;
        rest_on(&harness.registry, &b.id, LimitClass::Session, "all", resets * 1000);
        harness.plan_claude(&rotated_claude(TURN_OK));
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send_with("first", harness.recipe(live.clone()));
        until("the first turn", || harness.state().phase == Phase::Working).await;
        harness.send_with("second", harness.recipe(live.clone()));
        harness.release();
        until("the hold", || harness.rotations().len() == 1).await;
        assert_eq!(harness.rotations()[0].outcome, RotationOutcome::NoCandidate, "b was resting");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(harness.spawns().is_empty(), "and a re-check with nothing new to say moves nothing");

        harness.registry.accounts.limit_log().clear(&b.id, None).expect("clear");
        until("the continuation and the held message to reach b", || harness.after().len() == 2).await;
        let after = harness.after();
        assert!(after[0].contains("自动切换到 b@team"), "{}", after[0]);
        assert!(after[1].contains("second"), "{after:?}");
        let outcomes: Vec<RotationOutcome> = harness.rotations().iter().map(|rotation| rotation.outcome).collect();
        assert_eq!(outcomes, vec![RotationOutcome::Switched, RotationOutcome::NoCandidate], "newest first");
        assert_eq!(harness.tokens(), vec![fake_token("b@team")]);
    }

    /// L4: nothing freed up, and the held account's own limit lifted. The
    /// failed message is continued there before the queue goes.
    #[tokio::test]
    async fn a_lifted_hold_continues_the_failed_message_on_the_same_account() {
        let home = tempfile::tempdir().expect("tempdir");
        let (_, live) = accounts_in(home.path(), &[("a@work", true), ("relay", false)], None).await;
        // A five-hour window that resets in a couple of seconds, said by the
        // result too — the older CLI's epoch marker — so the rest is that short
        // rather than the class default a result with no time would give it.
        let resets = rest::now_ms() / 1000 + 2;
        let result = format!(
            r#"{{"type":"result","subtype":"success","is_error":true,"result":"Claude AI usage limit reached|{resets}","api_error_status":429}}"#
        );
        let harness = Harness::rotating_with(
            home,
            &refused_once("five_hour", resets, &result, ""),
            live.clone(),
            Duration::from_millis(50),
            Vec::new(),
            None,
        )
        .await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send_with("first", harness.recipe(live.clone()));
        until("the first turn", || harness.state().phase == Phase::Working).await;
        harness.send_with("second", harness.recipe(live.clone()));
        harness.release();
        until("the hold", || harness.rotations().len() == 1).await;

        until("the continuation, once the window resets", || harness.seen().len() == 2).await;
        let seen = harness.seen();
        assert!(
            seen[1].contains("[Droppen] 账号 a@work 的额度已恢复。上一条消息因额度用完没有完成，请从中断处继续。"),
            "{}",
            seen[1]
        );
        harness.release();
        until("then the held message", || harness.seen().len() == 3).await;
        assert!(harness.seen()[2].contains("second"), "{:?}", harness.seen());
        harness.release();
        until("the queue to drain", || {
            harness.state().queued == 0 && harness.state().phase == Phase::Idle
        })
        .await;
        assert!(harness.spawns().is_empty(), "the same process, the same account");
        assert_eq!(harness.rotations().len(), 1, "and no decision about the lift");
    }

    /// L5: somebody moved the session to another account by hand while the
    /// turn was running. The limit sends it there — not to whatever the pool
    /// would have chosen.
    #[tokio::test]
    async fn a_binding_changed_by_hand_is_where_a_limited_session_goes() {
        let home = tempfile::tempdir().expect("tempdir");
        let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true), ("c@home", true)], None).await;
        let c = accounts[2].clone();
        let resets = rest::now_ms() / 1000 + 7200;
        let harness = Harness::rotating(home, &refused_once("five_hour", resets, SESSION_LIMIT, ""), live.clone()).await;
        harness.plan_claude(&rotated_claude(TURN_OK));
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send_with("first", harness.recipe(live.clone()));
        until("the first turn", || harness.state().phase == Phase::Working).await;
        harness
            .registry
            .accounts
            .bind_session_account(SESSION, Some(&c.id), AccountProvider::Claude)
            .await
            .expect("bind by hand");
        harness.release();

        until("the continuation", || harness.after().len() == 1).await;
        assert!(harness.after()[0].contains("自动切换到 c@home"), "{}", harness.after()[0]);
        assert_eq!(
            harness.tokens(),
            vec![fake_token("c@home")],
            "the account that was chosen, not the pool's first"
        );
        let rotation = harness.latest_rotation().expect("a decision");
        assert_eq!((rotation.outcome, rotation.to.id.as_str()), (RotationOutcome::Switched, c.id.as_str()));
    }

    // ------------------------------------------------- ahead of a limit

    /// A claude whose every turn ends normally after an `allowed_warning`
    /// reading — the status a window nearing its cap carries — with the
    /// five-hour window at `utilization` and the week at 40 %. `before` is
    /// written ahead of the first turn's reading, for a background task.
    fn warns_every_turn(utilization: f64, resets_at: i64, before: &str) -> String {
        let week = resets_at + 4 * 86_400;
        format!(
            r#"
turns=0
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in *control_request*) continue ;; esac
  turns=$((turns + 1))
  await_go
  rm -f "$GO"
  if [ "$turns" = 1 ]; then
{before}
    :
  fi
  printf '%s\n' '{{"type":"rate_limit_event","rate_limit_info":{{"status":"allowed_warning","rateLimitType":"five_hour","resetsAt":{resets_at},"unifiedWindows":{{"five_hour":{{"resetsAt":{resets_at},"utilization":{utilization}}},"seven_day":{{"resetsAt":{week},"utilization":0.4}}}}}}}}'
  printf '%s\n' '{TURN_OK}'
done
"#
        )
    }

    /// File a fresh reading of `account` with the five-hour window at `five`
    /// and the week at `week`, both fractions, as a session on it would.
    fn read_account(registry: &InjectionRegistry, account: &str, five: f64, week: f64) {
        let resets = rest::now_ms() / 1000 + 3600;
        let info = serde_json::json!({
            "unifiedWindows": {
                "five_hour": {"resetsAt": resets, "utilization": five},
                "seven_day": {"resetsAt": resets + 86_400, "utilization": week},
            }
        });
        let windows = crate::quota::readings::windows_from_rate_limit_info(&info);
        registry
            .quota
            .ingest_session(crate::quota::UsageLogin::Account(account.to_string()), windows);
    }

    /// The whole of a move ahead of a limit, on the everyday case: a turn ends
    /// normally with the five-hour window at 96 % and a message queued behind
    /// it. The session moves to the account with room before that message
    /// goes in, and the message — not a continuation, because nothing failed
    /// — is the first thing the new account's process reads.
    #[tokio::test]
    async fn a_normal_turn_end_with_a_near_window_moves_the_session_ahead_and_the_next_message_runs_there() {
        let home = tempfile::tempdir().expect("tempdir");
        let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], None).await;
        let (a, b) = (accounts[0].clone(), accounts[1].clone());
        let resets = rest::now_ms() / 1000 + 5400;
        let harness = Harness::rotating(home, &warns_every_turn(0.96, resets, ""), live.clone()).await;
        read_account(&harness.registry, &b.id, 0.10, 0.20);
        harness.plan_claude(&rotated_claude(TURN_OK));
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send_with("first", harness.recipe(live.clone()));
        until("the first turn", || harness.state().phase == Phase::Working).await;
        harness.send_with("second", harness.recipe(live.clone()));
        until("the second message to queue", || harness.state().queued == 1).await;
        harness.release();

        until("the queued message to reach the new account", || {
            harness.after().len() == 1 && harness.state().phase == Phase::Idle
        })
        .await;
        assert_eq!(
            harness.injection().map(|run| run.state),
            Some(InjectionRunState::Ok),
            "nothing failed: the move was made between turns"
        );
        let seen = harness.seen();
        assert_eq!(seen.len(), 1, "the account running out got the first message and nothing else: {seen:?}");
        let after = harness.after();
        assert!(after[0].contains("second"), "the queued message is what goes in: {after:?}");
        assert!(!after[0].contains("[Droppen]"), "and no continuation prompt goes before it: {after:?}");

        assert_eq!(harness.respawns().len(), 1, "one respawn: {:?}", harness.spawns());
        assert!(
            harness.respawns()[0].contains(&format!("--resume {SESSION}")),
            "the same conversation, resumed"
        );
        assert_eq!(harness.tokens(), vec![fake_token("b@team")], "under the new account's token");
        assert_eq!(
            harness.registry.accounts.session_account_id(SESSION).await.as_deref(),
            Some(b.id.as_str()),
            "the binding moved with it"
        );

        let rotation = harness.latest_rotation().expect("the decision is on record");
        assert_eq!(rotation.trigger, RotationTrigger::Ahead);
        assert_eq!(rotation.outcome, RotationOutcome::Switched);
        assert_eq!((rotation.from.id.as_str(), rotation.to.id.as_str()), (a.id.as_str(), b.id.as_str()));
        assert_eq!(
            (rotation.class, rotation.scope.as_str()),
            (LimitClass::Session, "all"),
            "the five-hour window"
        );
        assert_eq!(rotation.used_percent, Some(96.0));
        assert_eq!(rotation.until_at.0, Some(rest::iso_ms(resets * 1000)), "when that window resets");
        assert_eq!(rotation.interrupted_tasks, 0);
        assert!(
            staged(&harness.recording(), crate::record::Stage::Rotate),
            "and the flight recording says so"
        );

        // On the new account nothing is near, and nothing moves again. The
        // next message is built for the binding, as the send route builds it.
        let under_b = Fingerprint {
            account: Some(b.id.clone()),
            credential: harness.registry.accounts.credential_hash(&b.id).await,
            ..live.clone()
        };
        harness.send_with("third", harness.recipe(under_b));
        until("the third message, on the same new process", || harness.after().len() == 2).await;
        assert_eq!(harness.respawns().len(), 1, "{:?}", harness.spawns());
        assert_eq!(harness.rotations().len(), 1, "{:?}", harness.rotations());
    }

    /// The point of moving early is that nothing is cut off, so a turn that
    /// leaves a background task running does not move the session — not at its
    /// end, and not before the next turn while the task still runs. Once the
    /// task is done, the next quiet point moves it.
    #[tokio::test]
    async fn a_move_ahead_waits_for_background_work_and_happens_at_the_next_quiet_point() {
        let home = tempfile::tempdir().expect("tempdir");
        let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], None).await;
        let b = accounts[1].clone();
        let resets = rest::now_ms() / 1000 + 5400;
        let finishes = r#"
dir=$(dirname "$GO")
(
  n=0
  while [ ! -f "$dir/tasks-done" ] && [ "$n" -lt 400 ]; do [ -d "$dir" ] || exit 97; sleep 0.02; n=$((n + 1)); done
  [ -f "$dir/tasks-done" ] && printf '%s\n' '{"type":"system","subtype":"background_tasks_changed","tasks":[]}'
) &
"#;
        let body = format!(
            "{finishes}{}",
            warns_every_turn(0.97, resets, &one_background_task("bt1", "Review the diff"))
        );
        let harness = Harness::rotating(home, &body, live.clone()).await;
        read_account(&harness.registry, &b.id, 0.05, 0.05);
        harness.plan_claude(&rotated_claude(TURN_OK));
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send_with("first", harness.recipe(live.clone()));
        until("the first turn", || harness.state().phase == Phase::Working).await;
        harness.send_with("second", harness.recipe(live.clone()));
        until("the second message to queue", || harness.state().queued == 1).await;
        harness.release();

        until("the second message to go to the same process", || harness.seen().len() == 2).await;
        assert_eq!(harness.state().background, 1, "the task is still running");
        harness.release();
        until("the second turn to end", || {
            harness.state().phase == Phase::Idle && harness.state().queued == 0
        })
        .await;
        assert!(
            harness.rotations().is_empty(),
            "no move while background work runs: {:?}",
            harness.rotations()
        );
        assert!(harness.spawns().is_empty(), "and no restart: {:?}", harness.spawns());

        harness.nudge("tasks-done");
        until("the task to finish", || harness.state().background == 0).await;
        harness.send_with("third", harness.recipe(live.clone()));
        until("the third message to reach the new account", || harness.after().len() == 1).await;
        assert!(harness.after()[0].contains("third"), "{:?}", harness.after());
        assert_eq!(harness.seen().len(), 2, "the old account never saw it: {:?}", harness.seen());
        assert_eq!(harness.tokens(), vec![fake_token("b@team")]);
        let rotation = harness.latest_rotation().expect("a decision");
        assert_eq!((rotation.trigger, rotation.outcome), (RotationTrigger::Ahead, RotationOutcome::Switched));
        assert_eq!(rotation.used_percent, Some(97.0));
    }

    /// Nowhere known to be better: the session stays, nothing is written down
    /// and nothing is pushed — a near window is not news on its own.
    #[tokio::test]
    async fn a_near_window_with_no_known_good_account_leaves_the_session_where_it_is() {
        let home = tempfile::tempdir().expect("tempdir");
        let (accounts, live) = accounts_in(home.path(), &[("a@work", true), ("b@team", true)], None).await;
        let b = accounts[1].clone();
        let resets = rest::now_ms() / 1000 + 5400;
        let harness = Harness::rotating(home, &warns_every_turn(0.99, resets, ""), live.clone()).await;
        // b is close to its own line: 88 % is not under 95 − 10.
        read_account(&harness.registry, &b.id, 0.88, 0.10);
        until("the init", || harness.state().phase == Phase::Idle).await;

        harness.send_with("first", harness.recipe(live.clone()));
        until("the first turn", || harness.state().phase == Phase::Working).await;
        harness.send_with("second", harness.recipe(live.clone()));
        harness.release();
        until("the second message on the same process", || harness.seen().len() == 2).await;
        harness.release();
        until("the queue to drain", || {
            harness.state().phase == Phase::Idle && harness.state().queued == 0
        })
        .await;
        assert!(harness.rotations().is_empty(), "{:?}", harness.rotations());
        assert!(harness.spawns().is_empty());
        assert_eq!(harness.injection().map(|run| run.state), Some(InjectionRunState::Ok));
    }

    // ------------------------------------------------------------ questions

    /// The two questions every question test asks, as the tool input carries
    /// them — and therefore exactly what an answer has to echo.
    fn colours_and_toppings() -> Value {
        serde_json::json!([
            {"question": "Which colour do you prefer?", "header": "Colour",
             "options": [{"label": "Red", "description": "Warm"}, {"label": "Blue", "description": "Cool"}],
             "multiSelect": false},
            {"question": "Which toppings?", "header": "Toppings",
             "options": [{"label": "Cheese"}, {"label": "Olives"}],
             "multiSelect": true}
        ])
    }

    /// The `assistant` line carrying the call and the `can_use_tool` request,
    /// as claude 2.1.295 writes them, for a script to `printf`.
    fn question_lines() -> [String; 2] {
        let request = serde_json::json!({
            "type": "control_request",
            "request_id": "req-q1",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "AskUserQuestion",
                "display_name": "AskUserQuestion",
                "input": {"questions": colours_and_toppings()},
                "tool_use_id": "toolu_q1",
                "requires_user_interaction": true,
            },
        });
        [
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_q1","name":"AskUserQuestion","input":{}}]}}"#.to_string(),
            request.to_string(),
        ]
    }

    /// The CLI with the prompt tool switched on: every message is answered with
    /// a question, and the turn then waits — with no timeout, as the real one
    /// does — for the response on stdin. The answer is echoed as the tool
    /// result the CLI builds from it; an interrupt is answered the way 2.1.295
    /// answers one with a question open: the request withdrawn, the turn
    /// failed.
    fn asks_a_question() -> String {
        let [call, request] = question_lines();
        format!(
            r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
    *control_response*)
       printf '%s\n' '{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_q1","content":"User has answered your questions"}}]}}}}'
       printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"thanks"}}]}}}}'
       printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}' ;;
    *control_request*)
       printf '%s\n' '{{"type":"control_cancel_request","request_id":"req-q1"}}'
       printf '%s\n' '{{"type":"result","subtype":"error_during_execution","is_error":true}}' ;;
    *)
       printf '%s\n' '{call}'
       printf '%s\n' '{request}' ;;
  esac
done
"#
        )
    }

    /// The line the driver wrote to answer `request_id`, parsed.
    fn response_to(seen: &[String], request_id: &str) -> Option<Value> {
        seen.iter()
            .filter(|line| line.contains("control_response"))
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|line| line.pointer("/response/request_id").and_then(Value::as_str) == Some(request_id))
    }

    fn blue_with_cheese_and_olives() -> questions::Answer {
        questions::Answer {
            answers: std::collections::BTreeMap::from([
                ("Which colour do you prefer?".to_string(), "Blue".to_string()),
                (
                    "Which toppings?".to_string(),
                    questions::join_choices(&["Cheese".to_string(), "Olives".to_string()]),
                ),
            ]),
            ..questions::Answer::default()
        }
    }

    /// The feature, end to end through the driver: the question holds the
    /// turn open — and the session *asking* while it is working — the answer
    /// goes back on stdin in the one shape the CLI takes, and the turn goes on
    /// to its own end.
    #[tokio::test]
    async fn a_question_holds_the_turn_open_and_its_answer_goes_back_on_stdin() {
        let harness = Harness::ordinary_with(&asks_a_question()).await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("pick for me");
        until("the question", || harness.state().question.is_some()).await;

        assert_eq!(harness.state().question.as_deref(), Some("req-q1"));
        assert_eq!(harness.state().phase, Phase::Working, "claude asks mid-turn, and the turn waits");
        let info: crate::sessions::ResidentInfo = harness.state().into();
        assert!(info.asking, "and the row says it is asking, while working");
        assert_eq!(harness.registry.open_question(SESSION).as_deref(), Some("req-q1"));
        let stored = harness.registry.questions().get("req-q1").expect("on record");
        assert_eq!((stored.state, stored.tool_use_id.as_deref()), (QuestionState::Pending, Some("toolu_q1")));
        assert_eq!(stored.questions, colours_and_toppings());

        // An answer to something that was not asked is refused, and changes
        // nothing.
        let wrong = questions::Answer {
            answers: std::collections::BTreeMap::from([("which colour?".to_string(), "Blue".to_string())]),
            ..questions::Answer::default()
        };
        assert!(matches!(
            harness.registry.answer_question(SESSION, "req-q1", Reply::Answer(wrong), None).await,
            AnswerOutcome::Invalid(_)
        ));
        assert!(response_to(&harness.seen(), "req-q1").is_none());

        let delivered = harness
            .registry
            .answer_question(SESSION, "req-q1", Reply::Answer(blue_with_cheese_and_olives()), Some("phone".into()))
            .await;
        let AnswerOutcome::Delivered(question) = delivered else {
            panic!("delivered: {delivered:?}")
        };
        assert_eq!(
            (question.state, question.answered_by.0.as_deref()),
            (QuestionState::Answered, Some("phone"))
        );

        until("the turn to go on and end", || harness.state().phase == Phase::Idle).await;
        let line = response_to(&harness.seen(), "req-q1").expect("the answer went in");
        assert_eq!(line["response"]["subtype"], "success");
        let response = &line["response"]["response"];
        assert_eq!(response["behavior"], "allow");
        assert_eq!(response["updatedInput"]["questions"], colours_and_toppings(), "echoed verbatim");
        assert_eq!(response["updatedInput"]["answers"]["Which colour do you prefer?"], "Blue");
        assert_eq!(response["updatedInput"]["answers"]["Which toppings?"], "Cheese, Olives");
        assert_eq!(harness.state().question, None);
        assert_eq!(harness.injection().map(|run| run.state), Some(InjectionRunState::Ok));

        // A second answer — another screen, a double tap — reaches nothing:
        // the driver is no longer holding the question, and the record says
        // who got there first.
        assert!(matches!(
            harness
                .registry
                .answer_question(SESSION, "req-q1", Reply::Answer(blue_with_cheese_and_olives()), Some("laptop".into()))
                .await,
            AnswerOutcome::Unknown
        ));
        let stored = harness.registry.questions().get("req-q1").expect("on record");
        assert_eq!((stored.state, stored.answered_by.as_deref()), (QuestionState::Answered, Some("phone")));
        assert_eq!(
            harness.seen().iter().filter(|line| line.contains("control_response")).count(),
            1,
            "one answer, once"
        );
    }

    /// "Use your judgement": refused, in words the agent can carry on from.
    #[tokio::test]
    async fn a_dismissed_question_is_refused_and_the_turn_goes_on() {
        let harness = Harness::ordinary_with(&asks_a_question()).await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("pick for me");
        until("the question", || harness.state().question.is_some()).await;
        let outcome = harness.registry.answer_question(SESSION, "req-q1", Reply::Dismiss, None).await;
        assert!(
            matches!(&outcome, AnswerOutcome::Delivered(question) if question.state == QuestionState::Dismissed),
            "{outcome:?}"
        );
        until("the turn to end", || harness.state().phase == Phase::Idle).await;
        let line = response_to(&harness.seen(), "req-q1").expect("the refusal went in");
        assert_eq!(line["response"]["response"], questions::deny(questions::DISMISSED));
    }

    /// Every prompt but a question is refused the moment it arrives, as
    /// `--permission-mode auto` refused it before the prompt tool existed: a
    /// write outside the workspace is not something a phone gets asked about.
    #[tokio::test]
    async fn every_other_permission_prompt_is_refused_at_once() {
        let request = serde_json::json!({
            "type": "control_request",
            "request_id": "req-w1",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "Write",
                "input": {"file_path": "/etc/hosts", "content": "x"},
                "tool_use_id": "toolu_w1",
                "decision_reason": "outside the workspace",
            },
        });
        let harness = Harness::ordinary_with(&format!(
            r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
    *control_response*)
       printf '%s\n' '{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_w1","is_error":true,"content":"denied"}}]}}}}'
       printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}' ;;
    *control_request*) : ;;
    *) printf '%s\n' '{request}' ;;
  esac
done
"#
        ))
        .await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("write it to /etc/hosts");
        until("the refusal and the turn's end", || {
            harness.state().phase == Phase::Idle && harness.seen().len() == 2
        })
        .await;
        let line = response_to(&harness.seen(), "req-w1").expect("refused");
        let response = &line["response"]["response"];
        assert_eq!(response["behavior"], "deny", "never allowed");
        let message = response["message"].as_str().expect("a message");
        assert!(
            message.contains("Write") && message.contains("outside the workspace") && message.contains("Droppen"),
            "{message}"
        );
        assert!(
            harness.registry.questions().get("req-w1").is_none(),
            "a permission prompt is not a question"
        );
        assert_eq!(harness.injection().map(|run| run.state), Some(InjectionRunState::Ok));
    }

    /// The two clocks that would throw a question's turn away, and must not.
    ///
    /// claude writes nothing while it waits — that is what blocked means — so
    /// the stall watchdog sees a silent turn; and the transcript, asked whether
    /// a turn is in flight, reads a tail ending on `AskUserQuestion` as a
    /// session waiting for input. Before the exemption, the two together let
    /// go of the turn after `DROPPEN_STALL_MS` and pushed the queue into the middle
    /// of it.
    #[tokio::test]
    async fn a_question_is_never_taken_for_a_stall_or_a_turn_that_ended() {
        let harness = Harness::start(
            &asks_a_question(),
            Duration::from_millis(60),
            Duration::from_millis(30),
            Duration::from_secs(3600),
        )
        .await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("pick for me");
        until("the question", || harness.state().question.is_some()).await;
        // The transcript as claude leaves it at a question, settled long ago.
        let transcript = harness.transcript();
        std::fs::write(
            &transcript,
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu_q1\",\"name\":\"AskUserQuestion\",\"input\":{}}]}}\n",
        )
        .expect("transcript");
        long_ago(&transcript);
        assert_eq!(
            crate::sessions::claude::claude_activity_status(&transcript).await,
            SessionStatus::Waiting,
            "the transcript would call this between turns"
        );
        // A message sent meanwhile waits behind the question's turn.
        harness.send("queued behind it");

        tokio::time::sleep(Duration::from_millis(400)).await;
        let state = harness.state();
        assert_eq!(state.phase, Phase::Working, "the turn is still in flight");
        assert!(!state.stalled, "waiting on the user is not being stuck");
        assert_eq!(state.question.as_deref(), Some("req-q1"));
        assert_eq!(state.queued, 1, "and the queue did not go into the middle of it");
        assert_eq!(harness.injection().map(|run| run.state), Some(InjectionRunState::Running));
        assert_eq!(harness.seen().len(), 1, "nothing else was written to claude");

        harness
            .registry
            .answer_question(SESSION, "req-q1", Reply::Answer(blue_with_cheese_and_olives()), None)
            .await;
        until("the queued message to go in after the answer", || {
            harness.seen().iter().any(|line| line.contains("queued behind it"))
        })
        .await;
    }

    /// A stop takes the question back with the turn: nobody may answer it into
    /// whatever comes next.
    #[tokio::test]
    async fn a_stop_cancels_the_question_it_was_blocked_on() {
        let harness = Harness::ordinary_with(&asks_a_question()).await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("pick for me");
        until("the question", || harness.state().question.is_some()).await;

        assert!(harness.registry.interrupt_resident(SESSION));
        until("the turn to end", || harness.state().phase == Phase::Idle).await;
        assert_eq!(harness.state().question, None);
        assert_eq!(
            harness.registry.questions().get("req-q1").map(|stored| stored.state),
            Some(QuestionState::Cancelled)
        );
        assert!(harness.injection().is_some_and(|run| run.interrupted));
        assert!(matches!(
            harness.registry.answer_question(SESSION, "req-q1", Reply::Dismiss, None).await,
            AnswerOutcome::Unknown
        ));
        assert!(response_to(&harness.seen(), "req-q1").is_none(), "no answer followed the stop");
    }

    /// A question is a promise from the process that asked it. When that
    /// process dies, the replacement `--resume`s with the call closed as
    /// interrupted and never asks again — so the question expires, and stays on
    /// record to be answered as an ordinary message.
    #[tokio::test]
    async fn a_question_expires_with_the_process_that_asked_it() {
        let [call, request] = question_lines();
        let harness = Harness::ordinary_with(&format!(
            r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  printf '%s\n' '{call}'
  printf '%s\n' '{request}'
  await_go
  rm -f "$GO"
  exit 9
done
"#
        ))
        .await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("pick for me");
        until("the question", || harness.state().question.is_some()).await;
        harness.release();

        until("the question to expire", || {
            harness.registry.questions().get("req-q1").map(|stored| stored.state) == Some(QuestionState::Expired)
        })
        .await;
        until("the resume", || harness.spawns().len() == 1).await;
        assert_eq!(harness.state().question, None, "the replacement is asking nothing");
        assert!(matches!(
            harness.registry.answer_question(SESSION, "req-q1", Reply::Dismiss, None).await,
            AnswerOutcome::Unknown
        ));
    }

    /// The history a server restart finds when it went down with a question
    /// open: the turn's prompt, the call, the request — and nothing since.
    fn history_with_an_open_question() -> String {
        let [call, request] = question_lines();
        [
            format!(r#"printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'"#),
            format!(r#"printf '%s\n' '{call}'"#),
            format!(r#"printf '%s\n' '{request}'"#),
        ]
        .join("\n")
    }

    /// A restart does not lose a question: the process is still blocked on it
    /// behind the shim, the replay finds it open — and opens the turn it blocks
    /// without asking the transcript, which would call it between turns — and
    /// it is answerable again.
    #[tokio::test]
    async fn a_question_outlives_a_server_restart() {
        let harness = Harness::start_attached(
            &history_with_an_open_question(),
            &asks_a_question(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
            Witness::None,
        )
        .await;
        until("the question to be held again", || harness.state().question.is_some()).await;
        assert_eq!(harness.state().phase, Phase::Working, "the turn it blocks is in flight");
        assert_eq!(
            harness.registry.questions().get("req-q1").map(|stored| stored.state),
            Some(QuestionState::Pending)
        );
        let outcome = harness
            .registry
            .answer_question(SESSION, "req-q1", Reply::Answer(blue_with_cheese_and_olives()), None)
            .await;
        assert!(matches!(outcome, AnswerOutcome::Delivered(_)), "{outcome:?}");
        until("the turn to end", || harness.state().phase == Phase::Idle).await;
        assert!(response_to(&harness.seen(), "req-q1").is_some());
    }

    /// The window the record exists for: the last server moved the record to
    /// `answered` and died before the answer reached claude's stdin. The replay
    /// still sees the question waiting, so the stored answer is written again —
    /// safely, because claude ignores a response it has already had.
    #[tokio::test]
    async fn an_answer_the_last_server_never_wrote_is_written_after_the_restart() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        InstanceDirs::new(&paths.data_dir(), crate::instance::DEFAULT_INSTANCE)
            .expect("valid name")
            .create()
            .expect("instance directories");
        let store = questions::Questions::new(crate::db::at(paths.instance_db()));
        let now = questions::now_ms();
        store.open("req-q1", SESSION, Some("toolu_q1"), &colours_and_toppings(), now);
        let delivery = questions::allow(&colours_and_toppings(), &blue_with_cheese_and_olives());
        store
            .resolve(
                "req-q1",
                &[QuestionState::Pending],
                QuestionState::Answered,
                Some(&blue_with_cheese_and_olives()),
                Some(&delivery),
                Some("phone"),
                now,
            )
            .expect("answered before the restart");
        // A question the record holds open and the history does not: settled
        // while nobody was listening, and expired by the replay.
        store.open("req-gone", SESSION, None, &colours_and_toppings(), now);

        let harness = Harness::build_in(
            home,
            &history_with_an_open_question(),
            &asks_a_question(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
            NEVER_SETTLES,
            Adoption {
                attach: Some(Witness::None),
                ..Adoption::default()
            },
        )
        .await;
        until("the stored answer to be written", || response_to(&harness.seen(), "req-q1").is_some()).await;
        let line = response_to(&harness.seen(), "req-q1").expect("written");
        assert_eq!(line["response"]["response"], delivery, "the very answer the user gave");
        until("the turn to end", || harness.state().phase == Phase::Idle).await;
        assert_eq!(harness.state().question, None, "nothing is left open");
        assert_eq!(
            harness.registry.questions().get("req-q1").map(|stored| stored.state),
            Some(QuestionState::Answered)
        );
        assert_eq!(
            harness.registry.questions().get("req-gone").map(|stored| stored.state),
            Some(QuestionState::Expired)
        );
    }

    /// A home with this session's question already on record, as the last
    /// server left it — the half of a restart the history cannot show.
    fn home_with_a_question(state: QuestionState) -> tempfile::TempDir {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        InstanceDirs::new(&paths.data_dir(), crate::instance::DEFAULT_INSTANCE)
            .expect("valid name")
            .create()
            .expect("instance directories");
        let store = questions::Questions::new(crate::db::at(paths.instance_db()));
        let now = questions::now_ms();
        store.open("req-q1", SESSION, Some("toolu_q1"), &colours_and_toppings(), now);
        if state != QuestionState::Pending {
            assert!(store.close("req-q1", state, now), "on record as {state:?}");
        }
        home
    }

    /// The shim's ring is finite, and a CLI blocked on a question can go on
    /// publishing housekeeping until the question itself has been lapped out of
    /// it. The replay then never sees the question; the history after the gap
    /// looks like nothing is happening, and the transcript — ending on the
    /// `AskUserQuestion` call — agrees. Only the record still knows: pending,
    /// so nobody answered it, so claude is still waiting. It is held open, the
    /// turn with it, and a message sent meanwhile waits behind the answer
    /// instead of going into a CLI that is blocked.
    #[tokio::test]
    async fn a_question_lost_in_a_replay_gap_is_still_held_open() {
        let [call, request] = question_lines();
        let history = [
            format!(r#"printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'"#),
            format!(r#"printf '%s\n' '{call}'"#),
            format!(r#"printf '%s\n' '{request}'"#),
            r#"i=0; while [ "$i" -lt 300 ]; do printf '%s\n' '{"type":"system","subtype":"commands_changed","commands":[]}'; i=$((i + 1)); done"#
                .to_string(),
        ]
        .join("\n");
        let harness = Harness::build_in(
            home_with_a_question(QuestionState::Pending),
            &history,
            &asks_a_question(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
            NEVER_SETTLES,
            Adoption {
                attach: Some(Witness::None),
                ring: Some(4096),
                transcript: Some(
                    r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_q1","name":"AskUserQuestion","input":{}}]}}"#
                        .to_string(),
                ),
                ..Adoption::default()
            },
        )
        .await;
        until("the question to be held open across the gap", || harness.state().question.is_some()).await;
        assert!(
            harness.recording().iter().any(|event| matches!(event, crate::record::Event::Gap { .. })),
            "the history really did lose its start"
        );
        assert_eq!(harness.state().phase, Phase::Working, "the turn the question blocks is open");
        assert_eq!(
            harness.registry.questions().get("req-q1").map(|stored| stored.state),
            Some(QuestionState::Pending)
        );

        harness.send("sent while it waits");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(harness.seen().is_empty(), "nothing is written into a blocked CLI: {:?}", harness.seen());
        assert_eq!(harness.state().queued, 1);

        let outcome = harness
            .registry
            .answer_question(SESSION, "req-q1", Reply::Answer(blue_with_cheese_and_olives()), None)
            .await;
        assert!(matches!(outcome, AnswerOutcome::Delivered(_)), "{outcome:?}");
        until("the message to go in once the turn is over", || {
            harness.seen().iter().any(|line| line.contains("sent while it waits"))
        })
        .await;
        assert!(response_to(&harness.seen(), "req-q1").is_some());
    }

    /// A control request this server has no handler for, left unanswered in
    /// the history — the last server died before declining it — is declined
    /// again when the replay ends, so the CLI is not left waiting on it.
    #[tokio::test]
    async fn a_control_request_nobody_answered_is_declined_after_a_restart() {
        let history = [
            format!(r#"printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'"#),
            r#"printf '%s\n' '{"type":"control_request","request_id":"hook-1","request":{"subtype":"hook_callback","callback_id":"x"}}'"#.to_string(),
        ]
        .join("\n");
        let harness = Harness::start_attached(
            &history,
            r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
done
"#,
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
            Witness::None,
        )
        .await;
        until("the request to be declined", || response_to(&harness.seen(), "hook-1").is_some()).await;
        let line = response_to(&harness.seen(), "hook-1").expect("declined");
        assert_eq!(line["response"]["subtype"], "error");
        assert!(line["response"]["error"].as_str().is_some_and(|error| error.contains("Droppen")));
    }

    /// A stop moves the record to `cancelled` before the interrupt is written.
    /// A server that dies between the two leaves claude still waiting on the
    /// question; the restart must finish the stop — not dismiss the question,
    /// which would let the turn carry on against what the user asked for.
    #[tokio::test]
    async fn a_stop_that_never_reached_claude_is_made_again_after_a_restart() {
        let harness = Harness::build_in(
            home_with_a_question(QuestionState::Cancelled),
            &history_with_an_open_question(),
            &asks_a_question(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
            NEVER_SETTLES,
            Adoption {
                attach: Some(Witness::None),
                ..Adoption::default()
            },
        )
        .await;
        until("the turn to be stopped", || {
            harness.state().phase == Phase::Idle && harness.injection().is_some_and(|run| run.interrupted)
        })
        .await;
        assert!(
            harness
                .seen()
                .iter()
                .any(|line| line.contains("control_request") && line.contains("\"subtype\":\"interrupt\"")),
            "the stop was made again: {:?}",
            harness.seen()
        );
        assert!(
            response_to(&harness.seen(), "req-q1").is_none(),
            "and the question was not answered instead"
        );
        assert_eq!(harness.state().question, None);
        assert_eq!(
            harness.registry.questions().get("req-q1").map(|stored| stored.state),
            Some(QuestionState::Cancelled)
        );
    }

    // -------------------------------------------------------------- drafts

    /// A CLI that thinks for as long as the test holds it: a count on stdout
    /// and a subagent's line beside it, then — on `$GO` — the finished text,
    /// its entry in the transcript and the `result`.
    fn thinks_until_released() -> String {
        let said = r#"{"type":"assistant","uuid":"u-early","message":{"id":"msg_early","role":"assistant","content":[{"type":"text","text":"said once the think was done"}]}}"#;
        let entry = format!(
            r#"{{"type":"assistant","uuid":"u-early","message":{{"id":"msg_early","role":"assistant","content":[{{"type":"text","text":"said once the think was done"}}]}},"timestamp":"2026-10-10T00:00:02.000Z","cwd":"/tmp","sessionId":"{SESSION}"}}"#
        );
        format!(
            r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
    *control_request*) : ;;
    *) printf '%s\n' '{{"type":"system","subtype":"thinking_tokens","estimated_tokens":40,"estimated_tokens_delta":40}}'
       printf '%s\n' '{{"type":"assistant","parent_tool_use_id":"toolu_child","uuid":"u-child","message":{{"content":[{{"type":"text","text":"a subagent"}}]}}}}'
       printf '%s\n' '{{"type":"system","subtype":"thinking_tokens","estimated_tokens":42,"estimated_tokens_delta":2}}'
       await_go
       rm -f "$GO"
       printf '%s\n' '{said}'
       printf '%s\n' '{entry}' >> "$TRANSCRIPT"
       printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}' ;;
  esac
done
"#
        )
    }

    /// What the session's draft holds right now.
    fn draft_of(harness: &Harness) -> crate::inject::drafts::Draft {
        harness
            .registry
            .draft_watch(Source::Claude, SESSION)
            .expect("a claude session has a draft")
            .borrow()
            .clone()
    }

    /// A think in flight is counted while the turn is held — one count, moved
    /// by each `thinking_tokens`, which a subagent's line in between does not
    /// end — and the `result` leaves nothing counted.
    #[tokio::test]
    async fn a_think_is_counted_until_the_turn_moves_on() {
        let harness = Harness::ordinary_with(&thinks_until_released()).await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        let before = draft_of(&harness).v;
        harness.send("think about it");
        until("the think to be counted", || draft_of(&harness).thinking == Some(42)).await;
        let draft = draft_of(&harness);
        assert!(draft.open);
        assert_eq!(draft.v, before + 2, "started at 40 and moved to 42, never ended in between: {draft:?}");

        harness.release();
        until("the turn to end", || {
            harness.state().phase == Phase::Idle && harness.injection().is_some_and(|run| run.state == InjectionRunState::Ok)
        })
        .await;
        let closed = draft_of(&harness);
        assert!(closed.is_empty() && !closed.open, "{closed:?}");
        assert_eq!(closed.turn, draft.turn);
    }

    /// A process that dies mid-think takes its count with it at once — there
    /// is no `result` to end it.
    #[tokio::test]
    async fn a_dead_process_takes_its_count_with_it() {
        let body = r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  printf '%s\n' '{"type":"system","subtype":"thinking_tokens","estimated_tokens":7,"estimated_tokens_delta":7}'
  await_go
  rm -f "$GO"
  exit 9
done
"#;
        let harness = Harness::ordinary_with(body).await;
        until("the init", || harness.state().phase == Phase::Idle).await;
        harness.send("doomed");
        until("the think to be counted", || draft_of(&harness).thinking == Some(7)).await;
        harness.release();
        until("the resume", || harness.spawns().len() == 1).await;
        until("the count to be cleared", || draft_of(&harness).is_empty()).await;
        assert!(!draft_of(&harness).open);
    }

    /// End to end through a real session feed: the page is sent the count as
    /// the one block of a `draft` frame — never what the agent said, which
    /// reaches it as an item once the transcript has it — then an empty draft
    /// once the think is over; and the turn's close is pushed rather than left
    /// for the poll.
    #[tokio::test]
    async fn a_page_is_sent_the_count_and_never_the_words() {
        let harness = Harness::ordinary_with(&thinks_until_released()).await;
        let prompt = format!(
            r#"{{"type":"user","uuid":"u-prompt","message":{{"role":"user","content":"think about it"}},"timestamp":"2026-10-10T00:00:00.000Z","cwd":"/tmp","sessionId":"{SESSION}"}}"#
        );
        std::fs::write(harness.transcript(), format!("{prompt}\n")).expect("transcript");
        let paths = Arc::new(Paths::new(harness._home.path().to_path_buf()));
        let instance = InstanceDirs::new(&paths.data_dir(), crate::instance::DEFAULT_INSTANCE).expect("valid name");
        let drive = Arc::new(crate::drive::Drive {
            auth: crate::auth::Auth::new(&paths, &instance, &crate::config::AuthSettings::default(), false),
            wizards: crate::accounts::setup_token::Captures::new(),
            injections: harness.registry.clone(),
            accounts: Arc::new(crate::accounts::store::AccountsStore::new(paths.clone())),
            quota: Arc::new(crate::quota::QuotaCache::new(paths.clone())),
            models: Arc::new(crate::models::ClaudeModelCache::new(paths.clone())),
            composer: crate::drive::composer_prefs::ComposerPrefsStore::new(paths.clone()),
            titles: crate::drive::titles::TitlesStore::new(paths.clone()),
            instance,
            paths,
        });
        let feeds = crate::feed::FeedRegistry::new();
        harness.registry.attach_listeners(feeds.clone(), crate::feed::global::GlobalFeed::new());
        until("the init", || harness.state().phase == Phase::Idle).await;
        let mut attached = crate::feed::attach_session_feed(&feeds, &drive, Source::Claude, SESSION, None)
            .await
            .expect("attached");

        let mut frames: Vec<Value> = Vec::new();
        let mut next = async |frames: &mut Vec<Value>| {
            let frame = tokio::time::timeout(Duration::from_secs(8), attached.attachment.next_frame())
                .await
                .unwrap_or_else(|_| panic!("no frame in time; so far {frames:?}"))
                .expect("the feed is open");
            let frame: Value = serde_json::from_str(&frame).expect("json");
            frames.push(frame.clone());
            frame
        };
        let carries_item = |frame: &Value| {
            matches!(frame["type"].as_str(), Some("delta" | "reset"))
                && frame["items"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| item["text"] == "said once the think was done"))
        };
        let counting = |frame: &Value| frame["type"] == "draft" && frame["blocks"].as_array().is_some_and(|blocks| !blocks.is_empty());

        harness.send("think about it");
        loop {
            let frame = next(&mut frames).await;
            if counting(&frame) && frame["blocks"][0]["thinkingTokens"] == 42 {
                assert_eq!(frame["blocks"], serde_json::json!([{"index": 0, "kind": "thinking", "thinkingTokens": 42}]));
                break;
            }
        }
        let released = std::time::Instant::now();
        harness.release();
        let ended =
            |frame: &Value| frame["type"] == "state" && frame["summary"]["resident"]["phase"] == "idle" && frame["injection"]["state"] == "ok";
        let mut ended_after = None;
        let (mut item, mut over) = (false, false);
        while !(item && over && ended_after.is_some()) {
            let frame = next(&mut frames).await;
            if ended(&frame) && ended_after.is_none() {
                ended_after = Some(released.elapsed());
            }
            item |= carries_item(&frame);
            over |= frame["type"] == "draft" && frame["blocks"] == serde_json::json!([]);
        }
        for frame in frames.iter().filter(|frame| frame["type"] == "draft") {
            assert!(!frame.to_string().contains("said once"), "a draft carries no words: {frame}");
        }

        // The close reaches the page as a state frame of its own, pushed by the
        // driver the moment the turn ends: the 2 s poll would get there too,
        // but a second and more later.
        let ended_after = ended_after.expect("the turn's end was pushed");
        assert!(
            ended_after < Duration::from_millis(1500),
            "the close took {ended_after:?} to reach the page"
        );
    }
}
