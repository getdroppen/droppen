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
//! …started once, held open by [`droppen_provider::shim`], and handed one line of JSON per
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
//! * **A server restart.** The process is behind [`droppen_provider::shim`], so killing
//!   this server does not touch it. The next boot re-attaches through the shim
//!   socket named in the journal, replays claude's whole stdout to work out
//!   where the session got to, and carries on — see [`ClaudeCodeRuns::attached`] and
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

use futures_util::future::BoxFuture;
use serde_json::Value;
use tokio::sync::mpsc;

use droppen_accounts::limits::{self, Verdict};
use crate::login::ClaudeCodeLogin;
use droppen_accounts::login::Login;
use crate::quota::RateLimitWindows;
use droppen_accounts::rest::{self, Observation};
use droppen_accounts::rotation::{self, AheadGuards, AheadPlan, Plan, Wait};
use droppen_accounts::store::AccountProvider;
use droppen_http::openapi_ext::Optional;
use droppen_base::procs;
use crate::record::{Recorder, SpawnSummary, Stage};
use droppen_sessions::account::{AccountRef, LimitClass, RotationOutcome, RotationTrigger};
use droppen_sessions::delivery::{PermissionOption, PermissionState, QuestionState, ReceiptStatus};
use droppen_sessions::{Injection, InjectionRunState, SessionStatus, Source};
use droppen_base::time::iso8601;

use droppen_provider::permissions::{self, PermissionSpec, Stored as StoredPermission};
use droppen_provider::questions::{self, PermissionRequest, Stored};
use droppen_provider::shim::{self, Link, Output, Sidecars, Spawn};
use droppen_provider::common::{busy_message, new_uuid, AnswerOutcome, PermissionOutcome, PermissionReply, Phase, QueueOutcome, Reply, ResidentState, StopTaskOutcome};
use droppen_provider::drafts::Draft;
use droppen_provider::gate::{ClaimError, SessionLease, SpawnGate};
use droppen_provider::{runs, Capabilities, DriverStatesBuilder, InjectError, InjectionRegistry, Provider, ProviderRuns, Run, RunEntry, RunHost, SendTo, Sent, SpawnPermit, StartAt, WrittenAt};
mod account_rotation;
mod background;
mod driver;
mod fingerprint;
mod interactive;
mod journal;
mod queue;
mod replay;
mod spawn;
mod wire;
#[cfg(any(test, feature = "test-support"))]
pub mod test_harness;
#[cfg(test)]
mod journal_tests;
#[cfg(test)]
mod tests;

pub(crate) use account_rotation::Hold;
pub use background::TASKS_ON_THE_WIRE;
use background::{BackgroundTasks, TaskProgress, TaskSeen};
pub(crate) use fingerprint::{Fingerprint, Recipe};
use interactive::{OpenPermission, OpenQuestion};
pub(crate) use journal::{ClaudeJournal, ClaudeRun, Queued};
use journal::is_false;
#[cfg(any(test, feature = "test-support"))]
pub use journal::ClaudeRowsExt;
use queue::AbsorbWatch;
use wire::{answered_in, classify, classify_value, clip, clip_to, last_lines, recipe_root, running_turn, user_line, user_line_with_blocks, ControlRequest, Event, StdinLine, Wake};

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
pub(crate) const RECOVERY_BACKOFF: [Duration; 3] = [Duration::from_secs(1), Duration::from_secs(5), Duration::from_secs(30)];

/// How long a steered message's turn waits for claude to prove it is running.
///
/// The one number the merged/deferred ambiguity is decided by, and the reason
/// it can be this short: a message claude buffered is answered with a
/// `system:init` of its own *immediately* — the CLI reads the next line of
/// stdin the instant the previous turn's `result` goes out, and the init is the
/// first thing it writes. Two seconds is three orders of magnitude past that
/// and still fast enough that a queue behind an absorbed message does not look
/// stuck. See [`Driver::settle_delivered`].
pub(crate) const STEER_SETTLE: Duration = Duration::from_secs(2);

/// How often a resident holding steered messages reads its transcript for them.
///
/// Only while one is outstanding, and only the bytes written since the last
/// look. A watched session hears sooner — its feed says when the file grows —
/// so this is the pace of the session list for a session nobody has open. See
/// [`Driver::absorb`].
pub(crate) const ABSORB_POLL: Duration = Duration::from_secs(1);

/// The shortest a settle will ever wait before asking again.
///
/// Only reached by a CLI that is talking right at the edge of the window: the
/// re-arm is otherwise "however long is left of the silence", and that figure
/// goes to zero as the last line approaches the deadline. A floor rather than a
/// special case, because the thing being prevented is a spin and not a wrong
/// answer.
const SETTLE_RECHECK: Duration = Duration::from_millis(25);

/// How often a quota reading with headroom may wake the residents holding
/// their queues after a limit; see [`ClaudeCodeRuns::recheck_holds`].
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


// ---------------------------------------------------------------- the mirror

/// What the send path and the routes hold on to.
pub(crate) struct Handle {
    tx: mpsc::UnboundedSender<Cmd>,
    shared: Arc<Mutex<ResidentState>>,
}

impl Handle {
    fn state(&self) -> ResidentState {
        self.shared.lock().expect("resident mirror poisoned").clone()
    }

    /// The mirror in the shape the API speaks in.
    pub(crate) fn info(&self) -> droppen_sessions::ResidentInfo {
        self.state().into()
    }
}

/// What the driver task accepts.
enum Cmd {
    /// A message for this session, with the process it wants.
    Deliver {
        pending: Box<Pending>,
        /// Answered once the message is in the journal, which is what the send
        /// route waits for before it says `200`; see
        /// [`ClaudeCodeRuns::deliver`].
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
        /// [`droppen_sessions::BackgroundTask::id`], which is claude's id for it.
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
    /// The user's decision on a tool permission prompt claude is blocked on.
    RespondPermission {
        request_id: String,
        reply: PermissionReply,
        by: Option<String>,
        respond: tokio::sync::oneshot::Sender<PermissionOutcome>,
    },
}

/// What one chat delivery should record once its line has reached the CLI.
///
/// The queue id is the row's identity, so a retry overwrites rather than adds;
/// `text` is the exact text block written, which the transcript parser hashes.
struct DeliveryRecord {
    id: String,
    text: String,
    images: Vec<String>,
    files: Vec<String>,
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
    /// Upload paths whose bytes become base64 image blocks in the stream-json
    /// user line. A tool-less chat's only image delivery; empty for a session,
    /// whose images are paths in [`Pending::text`] for Read.
    images: Vec<String>,
    /// Upload paths whose UTF-8 contents are inlined into the prompt at
    /// delivery, each under [`droppen_sessions::util::chat_file_block`]. Empty
    /// for a session, whose files are paths in [`Pending::text`] for Read.
    ///
    /// Paths rather than bytes: the journal holds these (a restart must deliver
    /// the same message), and a message's images can be megabytes each. They
    /// are re-fenced and re-read at delivery, so a swept upload fails the turn
    /// instead of sending an empty block.
    files: Vec<String>,
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
pub(crate) struct Continuation {
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
    #[cfg(any(test, feature = "test-support"))]
    fn new(text: &str, recipe: Recipe) -> Pending {
        Pending::with_id(new_uuid(), text, Vec::new(), Vec::new(), recipe)
    }

    /// The same, under a queue id the send path already chose — the one a
    /// client's receipt follows; see [`droppen_provider::receipts`] — with the
    /// upload paths a chat delivers as blocks or inlines at turn time.
    fn with_id(id: String, text: &str, images: Vec<String>, files: Vec<String>, recipe: Recipe) -> Pending {
        Pending {
            id,
            text: text.to_string(),
            images,
            files,
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
    fn recorded(&self, steered: bool) -> Queued {
        Queued {
            id: self.id.clone(),
            text: self.text.clone(),
            images: self.images.clone(),
            files: self.files.clone(),
            fingerprint: self.fingerprint.clone(),
            steered,
            rotation_attempt: self.rotation_attempt,
            continuation: self.continuation.clone(),
        }
    }

    /// This message as a row shows it.
    fn preview(&self, steered: bool) -> droppen_sessions::QueuedMessage {
        droppen_sessions::QueuedMessage {
            id: self.id.clone(),
            text: clip_to(&self.text, QUEUE_PREVIEW_CAP),
            steered,
        }
    }
}

// ------------------------------------------------------------- the send path

/// What a delivery did, for the caller to report.
pub(crate) struct Delivery {
    /// True when the message went in behind a turn that is still running.
    pub queued: bool,
}

/// How long a send may wait for its resident to journal the message before
/// the wait is logged. Normally it is milliseconds; the client calls a send
/// it heard nothing about for 30 s "unknown", so a slow journal is worth seeing.
const SLOW_JOURNAL: Duration = Duration::from_secs(2);

/// Wait for a resident's driver to say a message is on disk — exactly the
/// wait [`ClaudeCodeRuns::deliver`] has always made (it is what keeps an
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

/// Claude Code, as the provider registry knows it.
pub struct ClaudeCode {
    /// Its sessions' residents.
    runs: Arc<ClaudeCodeRuns>,
    /// Its CLI's models, per login.
    models: crate::models::ClaudeModels,
}

impl ClaudeCode {
    /// Claude Code, its runs on `registry`'s host.
    pub fn new(registry: Arc<InjectionRegistry>) -> Self {
        let models = crate::models::ClaudeModels::new(registry.host.paths.clone(), registry.host.accounts.clone());
        Self {
            runs: ClaudeCodeRuns::new(registry.host.clone(), &registry.instance, registry.decoration.clone()),
            models,
        }
    }

    /// Its runs, for a test that stands in for a driver or checks whose they
    /// are.
    #[cfg(any(test, feature = "test-support"))]
    pub fn runs(&self) -> &Arc<ClaudeCodeRuns> {
        &self.runs
    }

    /// Claude Code on runs a test made, timed as it needs.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn on(runs: Arc<ClaudeCodeRuns>, registry: &InjectionRegistry) -> Self {
        let models = crate::models::ClaudeModels::new(registry.host.paths.clone(), registry.host.accounts.clone());
        Self { runs, models }
    }
}

/// Claude Code's runs: the resident behind each session, and how their
/// drivers are timed, recorded and started. Kept here rather than on the
/// host they run on, whose shared parts — the paths, the runs journal, the
/// feeds — they reach through it.
pub struct ClaudeCodeRuns {
    /// What they run on.
    host: Arc<RunHost>,
    /// The resident claude behind each session, keyed by session id alone —
    /// there is no codex resident, so the source is not part of the key.
    pub(crate) residents: Mutex<std::collections::HashMap<String, Handle>>,
    /// How long a claude turn may make no progress before it is *marked*
    /// stalled. Nothing is ever killed for it; see [`self`].
    stall_after: Duration,
    /// How long a settings change may be parked behind claude's background work
    /// before the process is restarted anyway; `None` to wait for ever.
    ///
    /// A ceiling rather than a policy: parking is right, and the tasks a
    /// resident exists to protect can legitimately run for an hour. What this
    /// stops is the *unbounded* case — a background set that is wrong about
    /// work which is not there, holding a message nobody can stop or take back.
    /// See [`Driver::waited_long_enough`].
    await_background: Option<Duration>,
    /// `DROPPEN_ROTATE_AHEAD_PERCENT`: the used percent of a window at which a
    /// session is moved off its account at the next quiet point, before the
    /// limit refuses a turn; `None` when that is switched off. See
    /// [`Driver::rotate_ahead`].
    rotate_ahead: Option<f64>,
    /// Where claude drops a background task's output file.
    ///
    /// The CLI writes one `<task-id>.output` per background task under
    /// `<root>/<cwd-slug>/<session-id>/tasks/`, and streams into it while the
    /// task runs — so for a `local_bash` task, which has no subagent transcript
    /// anywhere, this is the *only* place on disk that says it is still alive.
    /// [`Driver::subagents_recently_active`] reads it for exactly that
    /// reason.
    ///
    /// A root rather than the directory itself because the slug in the middle is
    /// the working directory with its separators and its non-ASCII mangled, and
    /// reproducing that encoding is a guess; looking for the session id one level
    /// down is not. Overridable so a test can point it at a directory it owns.
    task_root: PathBuf,
    /// How long a resident may sit idle before it is retired, or `None` — the
    /// default — for never.
    ///
    /// Off by default because the resident model's whole promise is that the
    /// next message goes into a process that is already there and that nothing
    /// running in the background is ever cut off, and both are weakened by
    /// ending idle processes. It exists because a resident costs three or four
    /// hundred megabytes, and a machine with enough sessions open needs a way to
    /// say so. See [`Driver::idle_shutdown`].
    resident_idle: Option<Duration>,
    /// How long the recovery ladder waits before each restart of a resident
    /// that died unexpectedly: one entry per death inside the breaker's window,
    /// the last reused past the end.
    ///
    /// A field rather than the constant it defaults to so a test can walk the
    /// whole ladder — resume, fork, breaker — without sitting out real
    /// backoffs; production always runs [`RECOVERY_BACKOFF`].
    recovery_backoff: [Duration; 3],
    /// How long a turn opened for a *steered* message is given to prove it
    /// exists before the driver concludes claude answered the message inside
    /// the turn it was handed to.
    ///
    /// The whole of the merged/deferred ambiguity is timed by this, so it is a
    /// field for the same reason [`Self::recovery_backoff`] is: a test has to
    /// be able to watch both outcomes without sitting out the real one.
    /// Production runs [`STEER_SETTLE`], or `DROPPEN_STEER_SETTLE_MS`.
    steer_settle: Duration,
    /// How often a resident holding steered messages looks in the transcript
    /// for claude having taken them into the turn in flight.
    ///
    /// The backstop for a session nobody is watching: a watched one is told the
    /// moment its transcript grows — see [`droppen_provider::ProviderRegistry::transcript_grew`].
    /// Production runs [`ABSORB_POLL`], or `DROPPEN_ABSORB_POLL_MS`.
    absorb_poll: Duration,
    /// Where a session's flight recording goes, if it is being made at all.
    ///
    /// A factory rather than a recorder: one is opened per *resident*, lives on
    /// its driver, and is what carries the session's life into
    /// [`crate::record`]. See [`Driver::recorder`].
    recorders: crate::record::Recorders,
    /// What a claude spawn's environment gains before it starts: on the send
    /// path, and for the process an automatic rotation moves a session to,
    /// which a driver builds by itself.
    decoration: Arc<dyn droppen_provider::SpawnDecoration>,
    /// Instance-private directory holding `0600` per-session `--mcp-config`
    /// files for non-chat Claude sessions (`droppen-agents`).
    mcp_configs_dir: PathBuf,
    /// When a quota reading last woke the residents holding a queue; see
    /// [`ClaudeCodeRuns::recheck_holds`].
    reading_recheck: Mutex<Option<Instant>>,
}

impl ClaudeCodeRuns {
    /// None yet, on `host`, timed as the environment asks. `instance` says
    /// where the flight recordings go; `decoration` is what a claude spawn's
    /// environment gains before it starts.
    pub(crate) fn new(host: Arc<RunHost>, instance: &droppen_instance::instance::InstanceDirs, decoration: Arc<dyn droppen_provider::SpawnDecoration>) -> Arc<Self> {
        let recorders = crate::record::Recorders::new(host.paths.clone(), instance);
        let mcp_configs_dir = instance.mcp_configs();
        Arc::new(Self {
            host,
            residents: Mutex::new(std::collections::HashMap::new()),
            // Ten minutes of silence is a *report*, not a sentence: the only
            // thing that happens at this threshold is that the turn is marked.
            stall_after: droppen_provider::env_ms("DROPPEN_STALL_MS", Duration::from_secs(10 * 60)),
            // Half an hour: past every background job short enough that waiting
            // is the obvious answer, and well short of a session that is stuck.
            // Zero means "wait for ever", which is why this does not go through
            // `env_ms` either.
            await_background: match droppen_base::env::var("DROPPEN_AWAIT_BACKGROUND_MS")
                .and_then(|raw| raw.trim().parse::<u64>().ok())
            {
                Some(0) => None,
                Some(ms) => Some(Duration::from_millis(ms)),
                None => Some(Duration::from_secs(30 * 60)),
            },
            rotate_ahead: droppen_accounts::rotation::ahead_threshold_from_env(),
            // `/tmp`, not `TMPDIR`: claude writes here, and on macOS `TMPDIR` is
            // a per-process directory under `/var/folders` that it never uses.
            task_root: droppen_base::env::var("DROPPEN_TASK_ROOT").map(PathBuf::from).unwrap_or_else(|| {
                // SAFETY: `getuid` reads the calling process' own uid and cannot fail.
                PathBuf::from(format!("/tmp/claude-{}", unsafe { libc::getuid() }))
            }),
            // Zero, absent and unparseable all mean "never", which is why this
            // is the one timing here that does not go through `env_ms`: its
            // fallback is not a duration.
            resident_idle: droppen_base::env::var("DROPPEN_RESIDENT_IDLE_MS")
                .and_then(|raw| raw.trim().parse::<u64>().ok())
                .filter(|ms| *ms > 0)
                .map(Duration::from_millis),
            recovery_backoff: RECOVERY_BACKOFF,
            steer_settle: droppen_provider::env_ms("DROPPEN_STEER_SETTLE_MS", STEER_SETTLE),
            absorb_poll: droppen_provider::env_ms("DROPPEN_ABSORB_POLL_MS", ABSORB_POLL),
            recorders,
            decoration,
            mcp_configs_dir,
            reading_recheck: Mutex::new(None),
        })
    }
}

impl ProviderRuns for ClaudeCodeRuns {
    fn source(&self) -> Source {
        Source::Claude
    }

    fn host(&self) -> &Arc<RunHost> {
        &self.host
    }

    fn add_to(&self, states: &mut DriverStatesBuilder) {
        self.claude_driver_states(states);
    }

    fn cli_name(&self) -> &'static str {
        "claude"
    }

    fn readable(&self, record: &runs::RunRecord) -> bool {
        record.clone().decode::<ClaudeRun>().is_ok()
    }

    fn attach(self: Arc<Self>, record: runs::RunRecord, sidecars: Sidecars, link: Link, lease: SessionLease) {
        match record.decode::<ClaudeRun>() {
            Ok(record) => self.attached(&record, sidecars, link, lease),
            // Only a record it could read is handed over (`readable`).
            Err(error) => tracing::warn!(%error, "a run record cannot be read; that run will not be adopted"),
        }
    }

    fn was_cut_short<'a>(&'a self, record: &'a runs::RunRecord) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            match crate::sessions::claude::find_claude_session_file(&self.host.paths, &record.session_id).await {
                // No transcript at all: the CLI died before writing a line of
                // the turn it was given.
                None => true,
                Some(path) => crate::sessions::claude::claude_activity_status(&path).await == SessionStatus::Running,
            }
        })
    }

    fn last_write<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Option<Instant>> {
        Box::pin(async move { crate::sessions::claude::find_claude_session_file(&self.host.paths, id).await?.written_at().await })
    }

    fn question_holders(&self) -> Vec<String> {
        self.residents.lock().expect("residents poisoned").keys().cloned().collect()
    }

    fn mcp_config_names(&self, record: &runs::RunRecord) -> Vec<String> {
        let mut names = Vec::new();
        if let Some(shim) = record.shim.as_deref() {
            names.push(format!("claude-{shim}.json"));
        }
        // Backward compatibility for residents started by an older build that
        // keyed Claude MCP config files by session id.
        names.push(format!("claude-{}.json", record.session_id));
        names
    }
}


impl Provider for ClaudeCode {
    fn source(&self) -> Source {
        Source::Claude
    }

    /// claude stops to ask (`AskUserQuestion`), and the turn waits for the
    /// answer. An instant chat still runs with every tool disabled: its images
    /// arrive as base64 content blocks in the stream-json user line and its
    /// text files are inlined into the prompt, so nothing is read from disk by
    /// the model.
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            questions: true,
            permissions: true,
            chats: true,
            chat_attachments: true,
            chat_inline_attachments: true,
            schedules: true,
            rotations: true,
        }
    }

    fn run(&self) -> &dyn Run {
        self
    }

    fn runs(&self) -> Arc<dyn ProviderRuns> {
        self.runs.clone()
    }

    fn models(&self) -> &dyn droppen_provider::models::Models {
        &self.models
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    /// A session on the local login lives in the user's own root, and an
    /// account spawn runs in this instance's: it is carried over as a copy
    /// ([`crate::sessions::claude::continuation`]).
    fn continue_on_account<'a>(&'a self, id: &'a str, new_id: &'a str) -> BoxFuture<'a, Result<std::path::PathBuf, droppen_provider::ContinueError>> {
        Box::pin(crate::sessions::claude::continuation::copy_local_session(&self.runs.host.paths, id, new_id))
    }

    fn purge_archived<'a>(&'a self, paths: &'a droppen_instance::paths::Paths, cutoff_ms: i64) -> BoxFuture<'a, droppen_sessions::Purged> {
        Box::pin(async move {
            let live = crate::sessions::claude::live_claude_sessions(paths).await;
            let skip = |id: &str| {
                let state = self.resident(id);
                droppen_provider::has_work_in_flight(state.as_ref())
                    || (live.contains_key(id) && state.is_none())
            };
            crate::sessions::claude::archive::purge_archived(paths, cutoff_ms, &skip).await
        })
    }
}

impl Run for ClaudeCode {
    fn send<'a>(&'a self, gate: &'a SpawnGate, to: SendTo<'a>) -> BoxFuture<'a, Result<Sent, InjectError>> {
        Box::pin(self.runs.claude_send(gate, to))
    }

    fn start<'a>(&'a self, gate: &'a SpawnGate, at: StartAt<'a>) -> BoxFuture<'a, Result<String, InjectError>> {
        Box::pin(self.runs.claude_start(gate, at))
    }

    /// A resident publishes each turn in the runs map, so that is where a
    /// claude session's run is.
    fn injection(&self, id: &str) -> Option<Injection> {
        self.runs.host.run_injection(Source::Claude, id)
    }

    fn resident(&self, id: &str) -> Option<ResidentState> {
        Some(self.runs.residents.lock().expect("residents poisoned").get(id)?.state())
    }

    /// claude keeps a resident between turns, so the honest reading is its
    /// phase *and* its queue. A phase that is not `idle` means a turn is in
    /// flight or the process is still coming up; a queue that is not empty
    /// means work is already waiting behind it, and adding to that is how a
    /// backlog starts — remote-hand's queue never refuses a message, it waits,
    /// so nothing else would ever apply the brake.
    fn schedule_busy(&self, id: &str) -> Option<&'static str> {
        let state = self.resident(id)?;
        if state.phase != Phase::Idle {
            return Some("a turn is still in flight");
        }
        (state.queued > 0).then_some("messages are still queued")
    }

    /// Answers whether or not a resident is running: a page may be open on a
    /// session long before anything is sent to it, and has to be holding the
    /// channel the driver will write into when something is.
    fn draft_watch(&self, id: &str) -> Option<tokio::sync::watch::Receiver<Draft>> {
        Some(self.runs.host.drafts.subscribe(&self.runs.host.key_of(Source::Claude, id)))
    }

    /// Its resident looks for steered messages claude folded into the turn in
    /// flight now rather than at its next poll — see `Driver::absorb` — so the
    /// message leaves the queue on screen in the same moment it appears in the
    /// transcript. Cheap for a resident with nothing steered: it returns at once.
    fn transcript_grew(&self, id: &str) {
        if let Some(handle) = self.runs.residents.lock().expect("residents poisoned").get(id) {
            let _ = handle.tx.send(Cmd::Absorb { nudged: true });
        }
    }

    fn recheck_holds(&self) {
        self.runs.recheck_holds();
    }

    fn drop_queued<'a>(&'a self, id: &'a str, qid: &'a str) -> BoxFuture<'a, QueueOutcome> {
        Box::pin(self.runs.drop_queued(id, qid))
    }

    fn steer_queued<'a>(&'a self, id: &'a str, qid: &'a str) -> BoxFuture<'a, QueueOutcome> {
        Box::pin(self.runs.steer_queued(id, qid))
    }

    /// The resident, when this server holds one; otherwise a run adopted
    /// from the previous server.
    fn interrupt(&self, id: &str) -> bool {
        if self.holds(id) {
            return self.runs.interrupt_resident(id);
        }
        self.runs.host.interrupt_legacy(&*self.runs, id)
    }

    fn stop_background_task(&self, id: &str, task_id: &str) -> StopTaskOutcome {
        if !self.holds(id) {
            return StopTaskOutcome::NoResident;
        }
        self.runs.stop_resident_task(id, task_id)
    }

    fn open_question(&self, id: &str) -> Option<String> {
        self.runs.open_question(id)
    }

    fn answer_question<'a>(&'a self, id: &'a str, request_id: &'a str, reply: Reply, by: Option<String>) -> BoxFuture<'a, AnswerOutcome> {
        Box::pin(self.runs.answer_question(id, request_id, reply, by))
    }

    fn respond_permission<'a>(&'a self, id: &'a str, request_id: &'a str, reply: PermissionReply, by: Option<String>) -> BoxFuture<'a, PermissionOutcome> {
        Box::pin(self.runs.respond_permission(id, request_id, reply, by))
    }

    // Onto this server's hidden list: claude keeps no archive of its own.
    // Take the spawn gate to check that no active turn or queued message is
    // still running on this session before hiding it.
    fn archive<'a>(&'a self, gate: &'a SpawnGate, id: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let _permit = gate.permit().await;
            let busy = {
                let residents = self.runs.residents.lock().expect("residents poisoned");
                residents.get(id).is_some_and(|handle| {
                    let state = handle.state();
                    state.phase != Phase::Idle || state.queued > 0
                })
            };
            if busy {
                return Err("Claude session is running or has queued messages; stop it and clear the queue before archiving".to_string());
            }
            crate::sessions::claude::archive::archive_claude_session(&self.runs.host.paths, id)
                .await
                .map_err(|_| "archive failed".to_string())?;
            for record in self
                .runs
                .host
                .journal
                .read()
                .into_iter()
                .filter(|r| r.source == Source::Claude && r.session_id == id)
            {
                self.runs.remove_mcp_config(record.shim.as_deref());
            }
            // Legacy cleanup for residents started under an older build that
            // keyed Claude MCP configs by session id.
            let _ = std::fs::remove_file(self.runs.mcp_configs_dir.join(format!("claude-{id}.json")));
            Ok(())
        })
    }
}

impl ClaudeCode {
    /// Whether this server holds a resident for session `id`.
    fn holds(&self, id: &str) -> bool {
        self.runs.residents.lock().expect("residents poisoned").contains_key(id)
    }
}

impl ClaudeCodeRuns {
    /// The channel claude session `id`'s draft is published on.
    ///
    /// Its driver's [`droppen_provider::drafts::DraftWriter`] is the only thing that writes to it
    /// in a running server; a test of the feed stands in for that driver here.
    pub fn draft_channel(&self, id: &str) -> Arc<tokio::sync::watch::Sender<droppen_provider::drafts::Draft>> {
        self.host.drafts.channel(&self.host.key_of(Source::Claude, id))
    }

    /// The writer a driver for session `id` publishes its draft through.
    fn draft_writer(&self, id: &str) -> droppen_provider::drafts::DraftWriter {
        droppen_provider::drafts::DraftWriter::new(self.draft_channel(id))
    }

    /// Write a rotation decision down and tell everyone who should hear of it:
    /// the log line, the phone, the session's feed and the list.
    ///
    /// The one `info` line per decision is here, so every path that decides —
    /// a switch, nowhere to go, the cap — is logged the same way. Returns the
    /// record's id, for a switch that learns later how much it interrupted.
    fn record_rotation(&self, decision: &droppen_accounts::rotation::Decision, notice: &str) -> Option<i64> {
        let id = match self.host.accounts.rotation_log().record(decision) {
            Ok(id) => Some(id),
            Err(error) => {
                tracing::warn!(session = %decision.session_id, %error, "cannot record an account rotation");
                None
            }
        };
        tracing::info!(
            session = %decision.session_id,
            from = %decision.from.id,
            to = %decision.to.id,
            class = decision.class.as_str(),
            scope = %decision.scope,
            until = decision.until_ms.map(droppen_accounts::rest::iso_ms).as_deref().unwrap_or("-"),
            interrupted = decision.interrupted_tasks,
            outcome = decision.outcome.as_str(),
            trigger = decision.trigger.as_str(),
            used = ?decision.used_percent,
            "decided an account rotation"
        );
        // Known limitation: the failed turn behind this decision has
        // already sent its own "Run failed" notification a moment earlier, so
        // a limit reaches the phone twice — the failure, then what was done
        // about it. The second is the one that bypasses the throttle. A move
        // ahead of a limit has no failure before it, and reaches it once.
        // Every session on one account crosses the threshold together, so a
        // move ahead tends to come in a burst: N sessions from A to B within a
        // few minutes. One push per account pair per window says it; the rest
        // would only bury it. A limit-driven rotation always pushes.
        let ahead_burst = decision.trigger == droppen_sessions::account::RotationTrigger::Ahead
            && !droppen_provider::first_ahead_push(&decision.from.id, &decision.to.id, std::time::Instant::now());
        if !ahead_burst {
            if let Some(alerts) = self.host.alerts.get() {
                alerts.rotation(
                    self.host.paths.clone(),
                    Source::Claude,
                    decision.session_id.clone(),
                    decision.outcome,
                    decision.trigger,
                    notice.to_string(),
                );
            }
        }
        self.host.announce(Source::Claude, &decision.session_id);
        id
    }

    /// Claude's part of the session list's snapshot ([`ProviderRuns::add_to`]): every resident.
    pub(crate) fn claude_driver_states(&self, states: &mut DriverStatesBuilder) {
        for (id, handle) in self.residents.lock().expect("residents poisoned").iter() {
            states.resident(self.host.key_of(Source::Claude, id), handle.info());
        }
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
    /// [`ClaudeCodeRuns::start_resident`]). One posted to a resident that is
    /// already running is the driver's to write down, so this waits until it
    /// has — the same wait the queue routes and the codex send already make.
    /// Posting and returning, as this once did, answered `200` while the message
    /// was still sitting in the driver's inbox, and a `SIGKILL` in that window
    /// lost a message the user had been told was accepted.
    async fn deliver(
        self: &Arc<Self>,
        gate: &SpawnGate,
        id: &str,
        text: &str,
        images: Vec<String>,
        files: Vec<String>,
        recipe: Recipe,
        qid: Option<String>,
    ) -> Result<Delivery, InjectError> {
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
            let mut permit = gate.permit().await;
            let Some((queued, recorded)) = self.post_to_existing(id, &qid, text, &images, &files, &recipe) else {
                self.clone()
                    .start_resident(id, Box::new(Pending::with_id(qid, text, images, files, recipe)), &mut permit)
                    .await?;
                return Ok(Delivery { queued: false });
            };
            // The message is in a driver's inbox, so the gap the gate guards is
            // closed. Waiting for the journal under it would hold every other
            // session's sends behind one driver that is busy recovering.
            drop(permit);
            #[cfg(test)]
            self.host.step("posted to the resident", Source::Claude, Some(id));
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
    fn post_to_existing(
        &self,
        id: &str,
        qid: &str,
        text: &str,
        images: &[String],
        files: &[String],
        recipe: &Recipe,
    ) -> Option<(bool, tokio::sync::oneshot::Receiver<()>)> {
        let mut residents = self.residents.lock().expect("residents poisoned");
        let handle = residents.get(id)?;
        let pending = Box::new(Pending::with_id(qid.to_string(), text, images.to_vec(), files.to_vec(), recipe.clone()));
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
    async fn start_resident(self: Arc<Self>, id: &str, first: Box<Pending>, permit: &mut SpawnPermit<'_>) -> Result<(), InjectError> {
        // The whole-machine claim, held for as long as the process lives rather
        // than for one turn: a resident *is* the session's occupancy, and a
        // second instance starting its own against the same transcript is the
        // interleaving this lock exists to stop.
        permit.claim(Source::Claude, id).map_err(|error| match error {
            ClaimError::Busy { holder } => InjectError::message(busy_message(holder)),
            ClaimError::Io(error) => InjectError::Internal(format!("cannot claim this session: {error}")),
        })?;

        // A session that already has a transcript is resumed; one that does not
        // is being created, and `--session-id` is what makes the id the client
        // was already told about the one claude writes under.
        let selector = match crate::sessions::claude::find_claude_session_file(&self.host.paths, id).await {
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
        let spawned = self.spawn_link(permit, id, &recipe, selector, None, &[first.recorded(false)], &recorder).await?;

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
            permission: None,
            model: recipe.fingerprint.running_name().map(str::to_owned),
        }));
        let (tx, rx) = mpsc::unbounded_channel();
        self.residents.lock().expect("residents poisoned").insert(
            id.to_string(),
            Handle {
                tx: tx.clone(),
                shared: shared.clone(),
            },
        );
        #[cfg(test)]
        self.host.step("resident mapped", Source::Claude, Some(id));
        // The turn is visible from the moment the caller is told it was
        // accepted, and not from the moment claude gets around to saying it is
        // listening. Nothing may be written to a process before `system:init`,
        // so the driver cannot open the real turn yet — and a session that reads
        // as idle for the second or two a cold CLI takes to boot is a message
        // that, as far as the phone can tell, vanished. Replaced by the driver's
        // own entry as soon as there is one, and by the failure if the process
        // dies before there is.
        self.host.publish_run(Source::Claude, id, running_turn(id));

        let driver = Driver {
            runs: self.clone(),
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
            lease: permit.lease(),
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
            pending_permissions: Vec::new(),
            replay_asked: Vec::new(),
            replay_permissions: Vec::new(),
            replay_declines: Vec::new(),
            replay_gapped: false,
            draft: self.draft_writer(id),
            phase_moved: std::sync::atomic::AtomicBool::new(false),
        };
        tokio::spawn(driver.run());
        Ok(())
    }

    /// Best-effort cleanup of the per-resident `0600` `--mcp-config` file once
    /// its Claude process has exited, been replaced, failed to launch, or been
    /// archived. Keyed by shim id so a dying resident can never delete a
    /// racing replacement's config file.
    pub(crate) fn remove_mcp_config(&self, shim_id: Option<&str>) {
        if let Some(shim_id) = shim_id {
            let _ = std::fs::remove_file(self.mcp_configs_dir.join(format!("claude-{shim_id}.json")));
        }
    }

    /// Start one shim-held claude and record it, without touching the map.
    ///
    /// Shared by the first spawn and by the driver's respawn, which is what
    /// keeps the journal record and the sidecar naming identical between them.
    async fn spawn_link(
        &self,
        permit: &SpawnPermit<'_>,
        id: &str,
        recipe: &Recipe,
        selector: Selector,
        retiring: Option<&str>,
        queued: &[Queued],
        recorder: &Recorder,
    ) -> Result<Spawned, InjectError> {
        // On a fresh install no attachment has created this directory yet.
        // Do not claim the capability in the journal if preparing it failed.
        if recipe.fingerprint.uploads {
            tokio::fs::create_dir_all(self.host.paths.uploads_dir())
                .await
                .map_err(|error| InjectError::message(format!("cannot prepare the uploads directory: {error}")))?;
        }
        // Fresh sidecars for every spawn, never reused. The outgoing shim
        // unlinks its socket path on its way out and only checks that the path
        // still names *a* socket, so a replacement that bound the same path
        // would have it unlinked from under it.
        let sidecars = Sidecars::new(self.host.shim_dir.clone(), new_uuid());
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
        let mcp_config = self
            .claude_mcp_config(id, sidecars.id(), recipe.fingerprint.chat)
            .await;
        if let Some(path) = &mcp_config {
            spawn::insert_mcp_config_arg(&mut args, path);
        }

        // Built here rather than kept on the recipe: it is a reading of the
        // model and account the recipe already names, and a rotation's swapped
        // environment must be read, not a stale copy of the old one.
        let mut env = recipe.env.clone();
        spawn::enforce_auto_compaction(&mut env, recipe.fingerprint.running_name());

        #[cfg(test)]
        droppen_provider::testing::shims_run_as(crate::shims::SHIMS);
        let link = match shim::spawn_via_shim(Spawn {
            permit,
            sidecars: &sidecars,
            program: "claude",
            args: &args,
            workdir: recipe.workdir.as_deref(),
            env: &env,
            buffer_bytes: None,
        })
        .await
        {
            Ok(link) => link,
            Err(error) => {
                self.remove_mcp_config(Some(sidecars.id()));
                return Err(InjectError::Message(format!("failed to launch claude: {error}")));
            }
        };
        #[cfg(test)]
        self.host.step("claude started", Source::Claude, Some(id));

        let record = new_uuid();
        // The outgoing record goes only once the incoming process exists, so a
        // crash in between leaves something to clean up rather than nothing.
        if let Some(retiring) = retiring {
            self.host.journal.remove(retiring);
        }
        self.host.journal.add(runs::RunRecord {
            run_id: record.clone(),
            source: Source::Claude,
            session_id: id.to_string(),
            cwd: recipe.workdir.as_ref().map(|dir| dir.to_string_lossy().into_owned()),
            cli_pid: link.claude_pid(),
            started_at: iso8601(std::time::SystemTime::now()),
            pid_token: procs::start_token(link.claude_pid()),
            resident: true,
            shim: Some(sidecars.id().to_string()),
            payload: ClaudeRun {
                fingerprint: Some(recipe.fingerprint.clone()),
                // A process that has not been written to yet has no turn in
                // flight, whatever the one it replaces was in the middle of.
                turn_at: None,
                // What it does inherit is the backlog: the messages that made
                // this spawn happen, and that this process exists to answer.
                queued: queued.to_vec(),
                hold: None,
            },
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

    /// [`ClaudeCodeRuns::recheck_holds`] for a quota reading with headroom,
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
    pub(crate) async fn drop_queued(&self, id: &str, qid: &str) -> QueueOutcome {
        self.ask(id, |reply| Cmd::Drop { qid: qid.to_string(), reply }).await
    }

    /// Hand a queued message to claude now, ahead of the turn in flight.
    pub(crate) async fn steer_queued(&self, id: &str, qid: &str) -> QueueOutcome {
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
    pub(crate) fn interrupt_resident(&self, id: &str) -> bool {
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
    pub(crate) fn stop_resident_task(&self, id: &str, task_id: &str) -> StopTaskOutcome {
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
    /// caller goes on to [`ClaudeCodeRuns::answer_question`], which asks the
    /// driver and is told the truth.
    pub(crate) fn open_question(&self, id: &str) -> Option<String> {
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
    pub(crate) async fn answer_question(&self, id: &str, request_id: &str, reply: Reply, by: Option<String>) -> AnswerOutcome {
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

    /// Hand the user's decision on a tool permission prompt to the live
    /// resident for `id`.
    pub(crate) async fn respond_permission(&self, id: &str, request_id: &str, reply: PermissionReply, by: Option<String>) -> PermissionOutcome {
        let tx = {
            let residents = self.residents.lock().expect("residents poisoned");
            match residents.get(id) {
                Some(handle) => handle.tx.clone(),
                None => return PermissionOutcome::NoResident,
            }
        };
        let (respond, answer) = tokio::sync::oneshot::channel();
        let command = Cmd::RespondPermission {
            request_id: request_id.to_string(),
            reply,
            by,
            respond,
        };
        if tx.send(command).is_err() {
            return PermissionOutcome::NoResident;
        }
        answer.await.unwrap_or(PermissionOutcome::NoResident)
    }
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

impl ClaudeCodeRuns {
    /// Take over a resident this server did not start.
    ///
    /// The other half of [`ClaudeCodeRuns::start_resident`], for a boot that
    /// found a live `claude` in the journal. Everything the driver needs is
    /// rebuilt from what survived: the link from the shim socket, the settings
    /// from the record, the session's state from the replay the attach triggers.
    ///
    /// The claim is the caller's to have taken — adoption takes it before
    /// dialling, because a socket that does not answer still leaves a process
    /// running that nobody else may talk to.
    pub(crate) fn attached(self: &Arc<Self>, record: &runs::RunRecord<ClaudeRun>, sidecars: Sidecars, link: Link, lease: SessionLease) {
        // A record from before the fingerprint was written down says *unknown*,
        // and is carried through as unknown rather than as the default. The
        // default is "no flags at all", which is a real and very specific
        // description — one that nothing but a flagless message fits — so
        // reading it out of a record that says nothing is not caution, it is a
        // wrong answer that then throws the session's whole backlog away. See
        // [`Fingerprint::accepted_by`] for what unknown does instead, and
        // [`Driver::take_settings`] for how long it lasts.
        let live = record.payload.fingerprint.clone();
        let Inherited { delivered_ahead, queue } = inherited_queue(&record.session_id, &record.payload.queued, live.as_ref());
        let outstanding = delivered_ahead.len() + queue.len();
        // The ones that did not survive are the tail past the kept prefix: a
        // client that named one is told it failed rather than left waiting.
        for lost in record.payload.queued.iter().skip(outstanding) {
            self.host.note_receipt(Source::Claude, &record.session_id, &lost.id, ReceiptStatus::Failed);
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
            permission: None,
            model: live.as_ref().and_then(Fingerprint::running_name).map(str::to_owned),
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
        // bargain [`ClaudeCodeRuns::start_resident`] makes for a cold CLI.
        if record.payload.turn_at.is_some() || outstanding > 0 {
            self.host.publish_run(Source::Claude, &record.session_id, running_turn(&record.session_id));
        }
        // A shim that has written nothing has nothing to replay, which is the
        // everyday shape of a resident spawned and restarted-past before it said
        // a word. Such a driver starts live, exactly like a fresh spawn.
        let replaying = link.total() > 0;
        // The recording this session already has, re-opened and appended to:
        // one file per session for its whole life, however many servers and
        // however many processes that life spans.
        let recorder = self.recorders.open(&record.session_id);
        recorder.attached(record.cli_pid, record.shim.as_deref(), record.payload.turn_at, outstanding, link.total());
        let driver = Driver {
            runs: self.clone(),
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
            lease,
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
            pending_turn: record.payload.turn_at,
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
            hold: record.payload.hold.clone(),
            asked: Vec::new(),
            pending_permissions: Vec::new(),
            replay_asked: Vec::new(),
            replay_permissions: Vec::new(),
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
            turn = ?record.payload.turn_at,
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
fn inherited_queue(session: &str, queued: &[Queued], live: Option<&Fingerprint>) -> Inherited {
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
            images: message.images.clone(),
            files: message.files.clone(),
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
    run: Arc<RunEntry>,
    /// How far through claude's stdout this turn's prompt was written.
    ///
    /// The offset the journal records, kept here so that every rewrite of the
    /// record can restate it rather than having to remember whether it already
    /// had. See [`ClaudeRun::turn_at`].
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

// ---------------------------------------------------------------- the driver

/// The one task that owns a resident: its link, its queue and its state.
struct Driver {
    runs: Arc<ClaudeCodeRuns>,
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
    /// The session's claim: released when the process is gone, and not before.
    lease: SessionLease,
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
    /// True only for a driver built by [`ClaudeCodeRuns::attached`], and only
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
    /// Only ever set by [`ClaudeCodeRuns::attached`], and only until the
    /// replay walks past it. It is the half of the session's state that claude's
    /// stdout cannot carry — see [`ClaudeRun::turn_at`] — and it is
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
    /// The tool permission prompts claude is blocked on, oldest first.
    pending_permissions: Vec<OpenPermission>,
    /// What the history replayed so far leaves open: questions and permission
    /// prompts nobody has settled yet. Only meaningful while `replaying` is;
    /// settled by [`Driver::settle_questions`].
    replay_asked: Vec<OpenQuestion>,
    replay_permissions: Vec<OpenPermission>,
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
    /// [`droppen_provider::drafts`].
    draft: droppen_provider::drafts::DraftWriter,
    /// Set by [`Driver::set_phase`] when the phase moved, and taken by the
    /// next [`Driver::publish`], which pokes the feed again: see `set_phase`.
    /// Atomic only because the driver is shared by reference across awaits.
    phase_moved: std::sync::atomic::AtomicBool,
}
