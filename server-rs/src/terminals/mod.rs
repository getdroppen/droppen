//! Web terminals: PTYs the browser drives over a websocket.
//!
//! The engine lives here; the websocket and the four REST routes live in
//! [`routes`].
//!
//! # Where the pty lives
//!
//! Not here. Each terminal is a detached [`crate::pty_shim`] process that owns
//! the pty and the shell; this module owns a unix-socket link to it, a byte
//! offset into its output, and the browser-facing replay buffer. That is the
//! whole of "restart survival": the server can die and come back, and the only
//! thing it has to rebuild is state it can read off the instance directory.
//!
//! The wire the browser speaks is bare text frames of output out and
//! `{"type":"input"|"resize"}` JSON in, plus the four REST routes; the shim is
//! invisible on that wire except in two deliberate places: where output was
//! genuinely lost the client sees a counted [`crate::pty_shim::gap_note`]
//! rather than a seamless-looking splice, and where the link to a shim is down
//! it is told so rather than being left typing into a screen frozen at whenever
//! that link last worked.
//!
//! # Shape
//!
//! * one tokio task per terminal (`run_link`) owning the link, the 16 ms output
//!   coalescing, the exit note and the reaping timer;
//! * everything touched from more than one place ([`Replay`], the listener set,
//!   the title) sits behind its own small mutex;
//! * keystrokes and resizes go out as frames on an unbounded channel, so a
//!   handler never waits on a socket.
//!
//! Fan-out is a `Vec` of unbounded senders rather than a `tokio::sync::broadcast`,
//! deliberately: a broadcast drops frames for a client that falls behind, and a
//! terminal stream cannot be re-derived from a later frame the way the session
//! feed's can.

mod routes;

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use gotcha::Schematic;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::instance::InstanceDirs;
use crate::openapi_ext::Optional;
use crate::process_env::{self, Env};
use crate::pty_shim::client::{self, Link};
use crate::pty_shim::proto::{self, Frame, Meta, Resize};
use crate::pty_shim::sidecar::{self, Sidecars};
use crate::pty_shim::{self as shim, gap_note};
use crate::sessions::Paths;

pub use routes::{describe, routes};

/// How much output a (re)connecting client is repainted with.
const BUFFER_CAP: usize = 200 * 1024;
/// The pty emits many tiny chunks under load; coalescing them into one websocket
/// frame per interval keeps fast scrollers from flooding the client. The
/// interval throttles rather than delays: output after a quiet interval goes
/// out at once (a keystroke's echo does not wait 16 ms — on a direct path that
/// wait would be most of the echo time), and only what follows within the
/// interval is held for the next frame.
const FLUSH_INTERVAL: Duration = Duration::from_millis(16);
/// Past this much buffered output, flush immediately instead of waiting.
const FLUSH_CAP: usize = 64 * 1024;
/// How long an exited terminal stays attachable so clients see the exit note.
const REAP_AFTER: Duration = Duration::from_secs(10);
/// What every client sees when the process is gone.
const EXIT_NOTE: &str = "\r\n[process exited]\r\n";
/// What replaces the replay buffer when a wizard scrubs it.
const CLEARED_NOTE: &str = "\r\n[scrollback cleared: it held a credential]\r\n";
/// The terminal type every shell is told it is talking to.
const TERM: &str = "xterm-256color";
/// How much output may sit in one client's queue before it is disconnected.
///
/// The fan-out never *drops* a chunk — a terminal stream cannot be re-derived
/// from a later frame the way the session feed's can — so the only way to stop
/// a browser tab that has stopped reading from growing the server's heap without
/// bound is to stop serving it.
const LISTENER_QUEUE_CAP: usize = 4 * 1024 * 1024;
/// What a client that could not keep up sees before its socket closes.
pub const SLOW_NOTE: &str = "\r\n[client too slow]\r\n";
/// How many terminals may exist at once, exited-but-unreaped ones included.
///
/// Each one costs a pty, a shim process and up to [`BUFFER_CAP`] of replay, so a
/// client that loops on `POST /api/terminals` should hit a refusal rather than
/// the fd limit.
///
/// Public so the tests can pin the cap the create path and [`Registry::readopt_all`]
/// share, rather than restating the number and drifting from it.
pub const MAX_TERMINALS: usize = 64;
/// How long a freshly spawned shim is given to bind its socket.
const SPAWN_TIMEOUT: Duration = Duration::from_secs(10);
/// How long adoption waits for the repaint it asked for before listing the
/// terminal anyway.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long boot waits for the whole instance directory to be re-adopted.
///
/// One budget for all of them rather than one each, because this runs before the
/// listener is bound. A shim that has wedged costs a handshake timeout and then
/// a snapshot timeout before adoption gives up on it, so ten of them, one after
/// another, was minutes of a server that had not started answering yet.
const READOPT_BUDGET: Duration = Duration::from_secs(10);
/// The first and last delays of the reconnect backoff.
const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(10);
/// How long a killed terminal's tombstone is waited for before the sidecars are
/// swept anyway.
const TOMBSTONE_GRACE: Duration = Duration::from_secs(3);
/// How long a killed terminal's link task is given, once the tombstone is
/// there, to receive the shim's exit frame before [`reap_killed`] stops it.
///
/// The shim allows its reader half a second to drain the pty between the two
/// (`pty_shim::child::DRAIN_GRACE`); this leaves room for a busy machine on
/// top of that.
const EXIT_FRAME_GRACE: Duration = Duration::from_secs(2);

/// A live terminal, as the API describes it.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    /// Opaque id; also the websocket path segment.
    pub id: String,
    /// Display name, defaulted and renameable.
    pub title: String,
    /// Working directory the shell started in.
    pub cwd: String,
    /// When it was created, ISO-8601.
    pub created_at: String,
}

/// The `GET /api/terminals` response.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct TerminalList {
    /// Every terminal whose process is still running.
    pub terminals: Vec<TerminalInfo>,
}

/// The `POST /api/terminals` body.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Schematic)]
pub struct TerminalCreate {
    /// Working directory; defaults to the home directory.
    #[serde(default)]
    pub cwd: Optional<String>,
    /// Display name; defaults to `terminal N`.
    #[serde(default)]
    pub title: Optional<String>,
}

/// The `PATCH /api/terminals/{id}` body.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct TerminalRename {
    /// The new display name.
    pub title: String,
}

/// A scanner watching a terminal's raw output.
pub type DataHook = Box<dyn Fn(&str) + Send + Sync>;

/// A one-shot notification that a terminal's process is gone.
pub type ExitHook = Box<dyn FnOnce(i32) + Send>;

/// Why a terminal could not be created.
#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    /// The requested working directory escapes the home directory.
    #[error("path is outside the home directory")]
    Path,
    /// The shim could not be spawned, or would not answer.
    #[error("cannot start terminal")]
    Spawn,
    /// [`MAX_TERMINALS`] are already registered.
    #[error("too many terminals")]
    TooMany,
}

/// A terminal running something other than the user's login shell.
///
/// Internal on purpose: the REST create endpoint stays `{cwd, title}` so no
/// client can ask the server to run an arbitrary command. The account wizards
/// use this to put a `claude` sub-command on screen, and the resulting terminal
/// is an ordinary one — it lists, attaches, resizes and survives a restart
/// exactly like a shell terminal.
pub struct CreateSpec {
    /// Program to run.
    pub command: String,
    /// Arguments, not including `argv[0]`.
    pub args: Vec<String>,
    /// The complete environment; defaults to the scrubbed shell environment.
    pub env: Option<BTreeMap<String, String>>,
    /// Working directory; defaults to the home directory.
    pub cwd: Option<String>,
    /// Display name; defaults to `terminal N`.
    pub title: Option<String>,
    /// Raw pty output, before the flush coalescing — for output scanners.
    pub on_data: Option<DataHook>,
    /// Fires exactly once, on a natural exit or on [`Registry::kill_terminal`].
    pub on_exit: Option<ExitHook>,
    /// A pty size the server picks and no client may change.
    ///
    /// A pty is normally exactly as wide as the browser pane attached to it,
    /// which is the right answer for a terminal a person is typing into. It is
    /// the wrong answer for one a *scanner* is reading: the CLIs hard-wrap at the
    /// terminal width, so a phone-sized pane cuts every long value the wizard
    /// prints — a token included — into lines, and a value read back off the
    /// screen is then a value with a line break in the middle of it.
    ///
    /// So the size goes to the shim, which passes it to `openpty` *before* the
    /// child is spawned, and resize requests from clients are ignored for as long
    /// as the terminal lives. Widening it afterwards would not do: a child reads
    /// its width at startup, and by the time a resize lands it has already
    /// wrapped whatever it printed first.
    pub size: Option<(u16, u16)>,
}

impl std::fmt::Debug for CreateSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateSpec")
            .field("command", &self.command)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

impl CreateSpec {
    /// A spec that runs `command` with no arguments and every default.
    pub fn new(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            command: command.into(),
            args,
            env: None,
            cwd: None,
            title: None,
            on_data: None,
            on_exit: None,
            size: None,
        }
    }
}

/// What a terminal would replay to the next client to attach.
#[derive(Default)]
struct Replay {
    /// Recent output, capped at [`BUFFER_CAP`].
    buffer: String,
    /// Output collected since the last flush, not yet sent to listeners.
    pending: String,
}

/// What arrives on one attached client's queue.
#[derive(Debug, Clone)]
pub enum Chunk {
    /// Output to paint.
    Data(Arc<str>),
    /// This client fell too far behind and is being disconnected.
    TooSlow,
}

/// One attached client's queue.
struct Sink {
    id: u64,
    tx: mpsc::UnboundedSender<Chunk>,
    /// Bytes handed to `tx` that the client has not taken off it yet.
    queued: Arc<AtomicUsize>,
}

impl Sink {
    /// Queue `chunk`, or refuse it and disconnect a client that is not keeping up.
    ///
    /// Returns whether the sink is still worth keeping. The queue is unbounded
    /// at the channel level on purpose: the link must never block on a browser,
    /// so the backpressure is applied to the *client* instead.
    fn offer(&self, chunk: &Arc<str>) -> bool {
        if self.queued.load(Ordering::SeqCst) + chunk.len() > LISTENER_QUEUE_CAP {
            let _ = self.tx.send(Chunk::TooSlow);
            return false;
        }
        self.queued.fetch_add(chunk.len(), Ordering::SeqCst);
        self.tx.send(Chunk::Data(chunk.clone())).is_ok()
    }
}

/// Everyone currently watching one terminal.
#[derive(Default)]
struct Listeners {
    next_id: u64,
    sinks: Vec<Sink>,
}

/// One terminal: its shim, its replay buffer and its watchers.
struct Entry {
    /// Creation order. A `HashMap` iterates arbitrarily and rehashes as it
    /// grows, while the web renders `terminals` unsorted
    /// (`web/src/features/terminals/terminals-view.tsx`), so without this the
    /// user's tabs would reshuffle between polls — and after a restart, which is
    /// why it is also in the meta file.
    seq: u64,
    info: Mutex<TerminalInfo>,
    replay: Mutex<Replay>,
    listeners: Mutex<Listeners>,
    /// Frames for the shim, drained by the link task.
    outbound: mpsc::UnboundedSender<Frame>,
    /// Whether the link task is holding a live link to the shim right now.
    ///
    /// Nothing about a send says this, and that is what makes the failure quiet:
    /// `outbound` is unbounded, so a keystroke is accepted just as cheerfully
    /// while the link task sits inside [`relink`] backing off with nobody
    /// draining the channel — and just as cheerfully after the task has returned
    /// for good. A terminal in either state lists, attaches and repaints an
    /// entirely normal-looking screen while swallowing everything typed into it.
    ///
    /// A `watch` rather than a bare `AtomicBool` because a client that has been
    /// told the terminal is unreachable has to be *woken* when it stops being;
    /// see [`Attachment::link_changes`].
    linked: watch::Sender<bool>,
    /// The four files this terminal owns under the instance directory.
    sidecars: Sidecars,
    /// The shim process, for the fallback kill when the link is unusable.
    shim_pid: u32,
    /// The shell's pid — and process group — as the shim reported it.
    shell_pid: AtomicU32,
    /// The link task saw the process exit: an `x` frame, a tombstone or a dead
    /// shell. From then on the two pids above are history, and either may
    /// already belong to an unrelated process.
    exited: AtomicBool,
    /// Server-side exit hook; taken once so it runs exactly once.
    on_exit: Mutex<Option<ExitHook>>,
    /// Absolute offset of the output consumed from the shim.
    cursor: AtomicU64,
    /// The cursor value last written to disk.
    flushed: AtomicU64,
    /// Cancelling this detaches the link and leaves the shim running.
    detach: CancellationToken,
    /// [`Registry::kill_terminal`] ended this terminal.
    ///
    /// What tells the two reasons for cancelling [`Entry::detach`] apart.
    /// Shutdown detaches so that the next server can adopt a shell that is still
    /// running, and has nothing to tell anyone. A kill detaches only as a
    /// backstop, once the shell is already gone, and the clients still attached
    /// are owed the same exit note a natural exit gives them. See [`Entry::cut`].
    killed: AtomicBool,
    /// This terminal's width is the server's business, not a browser pane's.
    /// See [`CreateSpec::size`].
    size_locked: AtomicBool,
}

impl Entry {
    /// Add `chunk` to the not-yet-flushed output, returning its new length.
    fn push_pending(&self, chunk: &str) -> usize {
        let mut replay = self.replay.lock().expect("terminal replay poisoned");
        replay.pending.push_str(chunk);
        replay.pending.len()
    }

    /// Move the pending output into the replay buffer and out to every listener.
    fn flush(&self) {
        let mut replay = self.replay.lock().expect("terminal replay poisoned");
        self.flush_locked(&mut replay);
    }

    /// [`Entry::flush`], for a caller already holding the replay lock.
    ///
    /// The broadcast happens under that lock too. [`Entry::attach`] takes it to
    /// read the replay and register its listener as one step, so each chunk
    /// either goes into that replay or onto that listener's queue, never both
    /// and never neither.
    fn flush_locked(&self, replay: &mut Replay) {
        if replay.pending.is_empty() {
            return;
        }
        let chunk = std::mem::take(&mut replay.pending);
        replay.buffer.push_str(&chunk);
        truncate_front(&mut replay.buffer, BUFFER_CAP);
        self.broadcast(&chunk);
    }

    /// Send one already-flushed chunk to every attached client. Called with the
    /// replay lock held; see [`Entry::flush_locked`].
    fn broadcast(&self, chunk: &str) {
        let chunk: Arc<str> = Arc::from(chunk);
        let mut listeners = self.listeners.lock().expect("terminal listeners poisoned");
        listeners.sinks.retain(|sink| sink.offer(&chunk));
    }

    /// Mark the process gone and append the note every client repaints.
    fn note_exit(&self) {
        self.exited.store(true, Ordering::SeqCst);
        self.announce_exit();
    }

    /// Append the exit note every client repaints, and send it to those
    /// attached now.
    fn announce_exit(&self) {
        let mut replay = self.replay.lock().expect("terminal replay poisoned");
        replay.buffer.push_str(EXIT_NOTE);
        truncate_front(&mut replay.buffer, BUFFER_CAP);
        self.broadcast(EXIT_NOTE);
    }

    /// What the link task does when [`Entry::detach`] stops it before the shim
    /// has reported an exit.
    ///
    /// On shutdown the shell lives on, so this flushes and says nothing more. On
    /// a killed terminal the shell is gone, and leaving out the exit note would
    /// leave every attached client with a frozen pane that still looks live, on
    /// a terminal no longer in the list.
    ///
    /// The note is announced without marking the entry [`Entry::exited`]: a cut
    /// link has not seen the process end, and the reaper and
    /// [`Registry::kill_all`] may still have to signal it.
    fn cut(&self) {
        self.set_linked(false);
        self.flush();
        if self.killed.load(Ordering::SeqCst) && !self.exited.load(Ordering::SeqCst) {
            self.announce_exit();
        }
    }

    /// Run the exit hook, whether the process died on its own or was killed.
    fn finish(&self, exit_code: i32) {
        let hook = self.on_exit.lock().expect("terminal exit hook poisoned").take();
        let Some(hook) = hook else { return };
        // A wizard's bookkeeping must not take the terminal module down with it.
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || hook(exit_code))).is_err() {
            tracing::error!("a terminal exit hook panicked");
        }
    }

    /// Advance the consumed offset, recording it if it has moved far enough.
    fn advance(&self, bytes: u64) {
        let cursor = self.cursor.fetch_add(bytes, Ordering::SeqCst) + bytes;
        if cursor.saturating_sub(self.flushed.load(Ordering::SeqCst)) >= shim::CURSOR_FLUSH_EVERY {
            self.persist_cursor();
        }
    }

    /// Set the consumed offset outright — what a gap frame does.
    fn set_cursor(&self, cursor: u64) {
        self.cursor.store(cursor, Ordering::SeqCst);
        self.persist_cursor();
    }

    /// Write the consumed offset to disk now.
    fn persist_cursor(&self) {
        let cursor = self.cursor.load(Ordering::SeqCst);
        self.flushed.store(cursor, Ordering::SeqCst);
        self.sidecars.write_cursor(cursor);
    }

    /// Send a frame to the shim. Queued, never awaited: a handler must not wait
    /// on a socket.
    ///
    /// The frame is queued even when there is no link, deliberately: a reconnect
    /// that lands a second later should still deliver what was typed during it.
    /// The send's own result is not worth checking — see [`Entry::linked`] for
    /// why it is not the question — so callers who need to know whether anything
    /// will come of this ask [`Entry::is_linked`].
    fn tell(&self, frame: Frame) {
        let _ = self.outbound.send(frame);
    }

    /// Whether a frame sent now has a live link on the other end to carry it.
    fn is_linked(&self) -> bool {
        *self.linked.borrow()
    }

    /// Record that the link came up or went down.
    ///
    /// Only a real transition notifies, so a client watching does not blink once
    /// per failed retry through a long outage.
    fn set_linked(&self, linked: bool) {
        self.linked.send_if_modified(|current| std::mem::replace(current, linked) != linked);
    }

    /// Register a listener and hand back the client's handle on this terminal.
    ///
    /// Split out of [`Registry::attach`] so it can be reached with an entry and
    /// nothing else, which is what the unit tests have.
    fn attach(self: &Arc<Self>) -> Attachment {
        let (tx, rx) = mpsc::unbounded_channel();
        let queued = Arc::new(AtomicUsize::new(0));
        // The replay is read and the listener registered under one hold of the
        // replay lock, which every flush also broadcasts under. Done as two
        // steps, a flush between them sent its chunk to everyone but this
        // client, after a replay that did not have it yet. And output still
        // pending was sent twice: in the replay, then again by the flush.
        let mut buffered = self.replay.lock().expect("terminal replay poisoned");
        // Pending output goes out now, so the repaint is still complete.
        self.flush_locked(&mut buffered);
        let replay = buffered.buffer.clone();
        let listener_id = {
            let mut listeners = self.listeners.lock().expect("terminal listeners poisoned");
            listeners.next_id += 1;
            let listener_id = listeners.next_id;
            listeners.sinks.push(Sink {
                id: listener_id,
                tx,
                queued: queued.clone(),
            });
            listener_id
        };
        drop(buffered);
        Attachment {
            replay,
            entry: Arc::clone(self),
            listener_id,
            queued,
            rx,
        }
    }

    /// Signal the shell's process group and the shim, in that order.
    ///
    /// The order matters: killing the shim first would leave the shell orphaned
    /// with nothing holding its pty.
    ///
    /// Nothing at all once the exit has been seen: by then both pids may have
    /// been reused, and a `SIGKILL` to the group would land on a stranger.
    fn signal_directly(&self) {
        if self.exited.load(Ordering::SeqCst) {
            return;
        }
        let shell = self.shell_pid.load(Ordering::SeqCst);
        if shell > 1 {
            shim::child::kill_shell(shell, libc::SIGKILL);
        }
        terminate_shim(self.shim_pid);
    }
}

/// Every live terminal.
///
/// Lives in the application state rather than in a `static` — like
/// [`crate::feed::FeedRegistry`], and for the same reason: two servers in one
/// process (which is what the integration tests run) must not share or reap each
/// other's terminals. With shims that reason is stronger, not weaker: the
/// registry is scoped to one [`InstanceDirs`], and two registries on different
/// instances cannot see each other's shims at all.
pub struct Registry {
    /// Where a terminal starts when no `cwd` is given, and the boundary
    /// [`Registry::resolve_safe`] refuses to let one escape.
    paths: Arc<Paths>,
    /// The instance directory this registry owns. Every sidecar lives under it.
    instance: Arc<InstanceDirs>,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
    /// Terminals that have been killed but whose shims have not confirmed it yet.
    ///
    /// A killed terminal leaves the registry at once — it is gone as far as the
    /// API is concerned — but its shim is still being wound down by
    /// [`reap_killed`]. Keeping a reference here is what lets [`Registry::kill_all`]
    /// finish the job synchronously if the process is about to end, which is the
    /// difference between a test binary that leaves no shells behind and one
    /// that leaves one per kill.
    dying: Mutex<Vec<Arc<Entry>>>,
    /// Hands out [`Entry::seq`]; monotonic for the life of the *terminal*, not
    /// the process — adoption restores it from the meta files.
    created: AtomicU64,
}

impl Registry {
    /// An empty registry on `paths`, owning `instance`.
    pub fn new(paths: Arc<Paths>, instance: Arc<InstanceDirs>) -> Arc<Self> {
        Arc::new(Self {
            paths,
            instance,
            entries: Mutex::new(HashMap::new()),
            dying: Mutex::new(Vec::new()),
            created: AtomicU64::new(0),
        })
    }

    /// Every terminal whose process is still running.
    pub fn list(&self) -> Vec<TerminalInfo> {
        let entries = self.entries.lock().expect("terminal registry poisoned");
        let mut live: Vec<&Arc<Entry>> = entries.values().filter(|entry| !entry.exited.load(Ordering::SeqCst)).collect();
        live.sort_by_key(|entry| entry.seq);
        live.into_iter()
            .map(|entry| entry.info.lock().expect("terminal info poisoned").clone())
            .collect()
    }

    /// A terminal running the user's login shell.
    pub async fn create_terminal(self: &Arc<Self>, cwd: Option<String>, title: Option<String>) -> Result<TerminalInfo, TerminalError> {
        let mut spec = CreateSpec::new(login_shell(), vec!["-l".to_string()]);
        spec.cwd = cwd;
        spec.title = title;
        self.create_terminal_with(spec).await
    }

    /// A terminal running an arbitrary command; see [`CreateSpec`].
    ///
    /// The order is the recovery story in miniature: **meta first**, then the
    /// shim, then the link. A crash between the meta and the spawn leaves an
    /// identity with no socket, which adoption sweeps; a crash between the spawn
    /// and the link leaves a shim with an identity, which adoption *keeps*. The
    /// other order would lose a live shell.
    pub async fn create_terminal_with(self: &Arc<Self>, spec: CreateSpec) -> Result<TerminalInfo, TerminalError> {
        if self.count() >= MAX_TERMINALS {
            return Err(TerminalError::TooMany);
        }
        let workdir = match spec.cwd.as_deref().map(str::trim) {
            Some(cwd) if !cwd.is_empty() => spec.cwd.clone().expect("checked above"),
            _ => self.paths.home.to_string_lossy().into_owned(),
        };
        let mut env = spec.env.clone().unwrap_or_else(terminal_env);
        env.entry("TERM".to_string()).or_insert_with(|| TERM.to_string());

        let id = uuid::Uuid::new_v4().to_string();
        let seq = self.created.fetch_add(1, Ordering::SeqCst);
        let info = TerminalInfo {
            id: id.clone(),
            title: default_title(spec.title.as_deref(), self.count() + 1),
            cwd: workdir.clone(),
            created_at: crate::sessions::iso8601(std::time::SystemTime::now()),
        };
        let sidecars = Sidecars::new(self.instance.pty_shims(), &id);
        let meta = Meta {
            id: id.clone(),
            title: info.title.clone(),
            cwd: info.cwd.clone(),
            created_seq: seq,
            shell: spec.command.clone(),
            created_at: info.created_at.clone(),
        };
        sidecars.write_meta(&meta).await.map_err(|error| {
            tracing::error!(%error, "cannot record a terminal's identity");
            TerminalError::Spawn
        })?;
        sidecars.ensure_socket_dir().map_err(|error| {
            tracing::error!(%error, "cannot create the shim's socket directory");
            sidecars.remove_all();
            TerminalError::Spawn
        })?;

        let shim_pid = spawn_shim(&sidecars, &spec.command, &spec.args, &workdir, &env, spec.size)
            .await
            .map_err(|error| {
                tracing::error!(%error, command = %spec.command, "cannot spawn a pty shim");
                sidecars.remove_all();
                TerminalError::Spawn
            })?;

        let link = match await_link(&sidecars.socket(), 0).await {
            Ok(link) => link,
            Err(error) => {
                // The shim is running and holds a shell nothing will ever be
                // able to reach: it must not be left behind just because the
                // handshake failed.
                tracing::error!(%error, shim_pid, "a freshly spawned shim never answered; killing it");
                terminate_shim(shim_pid);
                sidecars.remove_all();
                return Err(TerminalError::Spawn);
            }
        };

        let (entry, outbound) = self.register(info.clone(), seq, sidecars, shim_pid, &link, spec.on_exit);
        // The size itself went to the shim, which opened the pty with it. All
        // that is left here is the latch: an attaching browser must not be able
        // to narrow a terminal whose width the server chose on purpose.
        if spec.size.is_some() {
            entry.size_locked.store(true, Ordering::SeqCst);
        }
        start_link(Arc::downgrade(self), entry, link, outbound, spec.on_data, Vec::new());
        Ok(info)
    }

    /// Re-attach to every shim this instance left behind, and return how many
    /// were adopted before [`READOPT_BUDGET`] ran out.
    ///
    /// Runs once at boot, *before* the listener exists, which is what shapes it:
    /// every terminal is adopted concurrently and the whole scan shares one
    /// budget. Serially, a directory of shims that will not answer cost a
    /// handshake timeout each before the next one was even tried, and the server
    /// bound nothing in the meantime — the failure a user sees is not a slow tab
    /// but a server that never starts.
    ///
    /// Anything still in flight when the budget runs out is left running rather
    /// than abandoned. It registers itself when the shim finally answers (see
    /// [`Registry::readopt_one`]), so a terminal adopted at t+12 s is late to
    /// `GET /api/terminals` and nothing worse.
    ///
    /// Concurrency cannot reorder the user's tabs, because the order was never
    /// registration order: [`Registry::list`] sorts on [`Entry::seq`], which
    /// comes off the meta file, and [`identify`] reads and orders every identity
    /// — the cap below then applies to that order — before the first handshake
    /// is attempted.
    pub async fn readopt_all(self: &Arc<Self>) -> usize {
        let dir = self.instance.pty_shims();
        let ids = sidecar::scan(&dir).await;
        let claimed: Vec<Sidecars> = ids.iter().map(|id| Sidecars::new(&dir, id)).collect();
        // Before anything else: sockets nothing claims are debris from a crash
        // between spawning a shim and recording what it was.
        sidecar::sweep_orphan_sockets(&dir, &claimed).await;

        let mut candidates = identify(&dir, &ids).await;
        // Taken from the identities rather than from what is adopted, and before
        // any of them is attached: a terminal created while a slow adoption is
        // still in flight must still sort after everything on disk.
        let highest = candidates.iter().map(|(meta, _)| meta.created_seq + 1).max().unwrap_or(0);
        self.created.fetch_max(highest, Ordering::SeqCst);

        // The same ceiling the create path enforces. An instance directory can
        // hold more than [`MAX_TERMINALS`] identities — a server that crashed
        // often enough leaves them — and adopting all of them would put the
        // registry somewhere `create_terminal_with` can never take it. The
        // extras keep their sidecars: their shells are alive, and removing the
        // identity is the one thing that would orphan one for good, so a boot
        // with room adopts them instead.
        let room = MAX_TERMINALS.saturating_sub(self.count());
        if candidates.len() > room {
            let over = candidates.split_off(room);
            tracing::warn!(
                over = over.len(),
                cap = MAX_TERMINALS,
                "more surviving terminals than the cap allows; leaving the newest for a later boot"
            );
        }

        let adopted = Arc::new(AtomicUsize::new(0));
        let tasks: Vec<_> = candidates
            .into_iter()
            .map(|(meta, sidecars)| {
                let registry = self.clone();
                let adopted = adopted.clone();
                tokio::spawn(async move {
                    if registry.readopt_one(meta, sidecars).await {
                        adopted.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        // Dropping the handles detaches rather than cancels, which is the whole
        // point: what the budget stops *waiting* for still runs to completion.
        if tokio::time::timeout(READOPT_BUDGET, futures_util::future::join_all(tasks)).await.is_err() {
            tracing::warn!(
                budget = ?READOPT_BUDGET,
                "adoption is taking longer than its budget; the rest will land while the server is already listening"
            );
        }
        adopted.load(Ordering::SeqCst)
    }

    /// Re-attach to one surviving shim, and say whether it became a terminal.
    ///
    /// Authority first and attachment last, because each step can only be
    /// trusted if the one before it held:
    ///
    /// 1. **tombstone** — the shell already exited. Report it and sweep; there
    ///    is nothing to attach to and a socket may still be lingering.
    /// 2. **socket** — absent, with no tombstone, means the shim died without
    ///    saying so. Report an unknown exit and sweep.
    /// 3. **corpse guards** — only now attach, and refuse if a tombstone
    ///    appeared during the handshake or if the pid the shim named is dead.
    ///    Both are the same failure: a shim minding a corpse would otherwise be
    ///    adopted as a working terminal that never produces another byte.
    ///
    /// The identity ([`identify`]) is read before any of this, so what arrives
    /// here is already known to be a terminal rather than debris.
    async fn readopt_one(self: &Arc<Self>, meta: Meta, sidecars: Sidecars) -> bool {
        let id = meta.id.clone();
        if let Some(tombstone) = sidecars.read_tombstone().await {
            tracing::info!(%id, exit_code = tombstone.exit_code, at = %tombstone.at, "a surviving terminal had already exited");
            sidecars.remove_all();
            return false;
        }
        if sidecars.tombstoned() {
            tracing::info!(%id, "a surviving terminal had already exited (status unreadable)");
            sidecars.remove_all();
            return false;
        }
        if !sidecars.socket().exists() {
            tracing::warn!(%id, "a pty shim went away without leaving an exit file");
            sidecars.remove_all();
            return false;
        }

        let cursor = sidecars.read_cursor().await;
        let link = match client::connect(&sidecars.socket(), cursor).await {
            Ok(link) => link,
            Err(error) => {
                tracing::warn!(%id, %error, "a pty shim would not answer; sweeping it");
                sidecars.remove_all();
                return false;
            }
        };
        // The guards run *after* the hello, because one of them needs what
        // the hello carries, and the other can only have become true during
        // it.
        if sidecars.tombstoned() || !crate::procs::alive(link.shell_pid) {
            tracing::warn!(%id, shell_pid = link.shell_pid, "refusing to adopt a shim minding a dead shell");
            drop(link);
            sidecars.remove_all();
            return false;
        }

        let info = TerminalInfo {
            id: meta.id.clone(),
            title: meta.title.clone(),
            cwd: meta.cwd.clone(),
            created_at: meta.created_at.clone(),
        };
        // Registered before the repaint is asked for, which is what makes a slow
        // adoption merely late rather than lost: from the shim's hello onwards
        // the terminal is in the registry, whether or not [`READOPT_BUDGET`] is
        // still running.
        let (entry, outbound) = self.register(info, meta.created_seq, sidecars, 0, &link, None);
        entry.cursor.store(cursor, Ordering::SeqCst);
        entry.flushed.store(cursor, Ordering::SeqCst);
        // The repaint is fetched here, before the link task takes over, so the
        // first browser to arrive gets a screen rather than an empty pane that
        // fills in later.
        let (link, carry) = adopt_snapshot(&entry, link).await;
        start_link(Arc::downgrade(self), entry, link, outbound, None, carry);
        true
    }

    /// Give a terminal a new display name.
    pub async fn rename_terminal(&self, id: &str, title: &str) -> Option<TerminalInfo> {
        let entry = self.live(id)?;
        let info = {
            let mut info = entry.info.lock().expect("terminal info poisoned");
            if !title.trim().is_empty() {
                info.title = title.trim().to_string();
            }
            info.clone()
        };
        // The meta file is what a restart rebuilds the list from, so a rename
        // that only changed memory would be undone by the next one.
        if let Some(mut meta) = entry.sidecars.read_meta().await {
            meta.title = info.title.clone();
            let _ = entry.sidecars.write_meta(&meta).await;
        }
        Some(info)
    }

    /// Drop what a terminal would replay to the next client to attach.
    ///
    /// The wizards call this the moment a credential appears on screen: the user
    /// watching live is meant to see it, but it must not sit in a 200 KiB buffer
    /// waiting to be repainted for whoever attaches later.
    ///
    /// The shim's own ring is out of reach — the protocol has no "forget" frame
    /// and is deliberately frozen — so the cursor is jumped to what has been
    /// consumed and written out immediately instead. That is what stops a
    /// restart from replaying the credential back out of the ring; the shim's
    /// memory still holds it until the wizard kills the terminal moments later,
    /// which is the same window the pty itself always had.
    pub fn clear_terminal_replay(&self, id: &str) -> bool {
        let Some(entry) = self.get(id) else { return false };
        {
            let mut replay = entry.replay.lock().expect("terminal replay poisoned");
            // Anything already coalesced is dropped with it; a flush that fires
            // afterwards finds nothing pending and is a no-op.
            replay.pending.clear();
            replay.buffer.clear();
            replay.buffer.push_str(CLEARED_NOTE);
        }
        entry.persist_cursor();
        true
    }

    /// Type into a terminal without attaching to it, and say whether it is live.
    ///
    /// [`Attachment::write`] is the browser's way in and registers a listener to
    /// get there. The account wizards need neither: they answer a prompt on the
    /// user's behalf — the OAuth code, typed by the server because the browser
    /// asking has no terminal of its own — and have no interest in the output.
    pub fn write_terminal(&self, id: &str, data: &str) -> bool {
        let Some(entry) = self.live(id) else { return false };
        entry.tell(Frame::raw(proto::INPUT, data.as_bytes().to_vec()));
        true
    }

    /// Kill a terminal's process. The exit hook fires here, not on the process's
    /// own exit, so a caller waiting on it is not left hanging.
    pub fn kill_terminal(self: &Arc<Self>, id: &str) -> bool {
        let Some(entry) = self.entries.lock().expect("terminal registry poisoned").remove(id) else {
            return false;
        };
        // A terminal whose process has already exited is only still registered
        // so its clients can read the exit note (`REAP_AFTER`), and closing its
        // tab in that window lands here. Taking it off the registry is all there
        // is to do: its link task has swept the sidecars and fired the exit hook
        // with the real status. A reaper would wait out a tombstone that is
        // already gone and then signal pids that may have been reused.
        if entry.exited.load(Ordering::SeqCst) {
            return true;
        }
        // The identity goes first and synchronously: everything after this is
        // asynchronous, and a crash in the middle must not leave behind a meta
        // file that the next boot would adopt as a terminal the user closed.
        entry.sidecars.remove_server_owned();
        entry.killed.store(true, Ordering::SeqCst);
        entry.tell(Frame::empty(proto::KILL));
        // -1: killed, so there is no exit status the wizard could report.
        entry.finish(-1);
        self.dying.lock().expect("terminal registry poisoned").push(entry.clone());
        tokio::spawn(reap_killed(Arc::downgrade(self), entry));
        true
    }

    /// Attach a client, returning the replay it should repaint with plus the
    /// handle that yields every later chunk.
    pub fn attach(&self, id: &str) -> Option<Attachment> {
        Some(self.live(id)?.attach())
    }

    /// Let go of every terminal without killing anything, and return how many.
    ///
    /// This is the point of the whole subsystem: the shell belongs to a shim
    /// that is *supposed* to outlive this process, and killing it here would
    /// throw away exactly what the user restarted the server to keep. So shutdown
    /// drops links and nothing else.
    ///
    /// The cursor is deliberately not written on the way out either — see
    /// [`crate::pty_shim::CURSOR_FLUSH_EVERY`].
    pub fn shutdown_all(&self) -> usize {
        let entries = self.drain();
        let count = entries.len();
        for entry in entries {
            entry.detach.cancel();
        }
        count
    }

    /// Kill every terminal and sweep its sidecars.
    ///
    /// Not part of shutdown, and deliberately not reachable from the API: it
    /// exists so a test can leave no shim processes behind. Signals rather than
    /// frames, so it works whether or not a link is healthy and needs no runtime
    /// to make progress.
    pub fn kill_all(&self) -> usize {
        let mut entries = self.drain();
        let count = entries.len();
        // Terminals that were killed a moment ago and whose shims are still
        // winding down count too: this may be the last thing this process does.
        entries.extend(self.dying.lock().expect("terminal registry poisoned").drain(..));
        for entry in entries {
            entry.detach.cancel();
            entry.signal_directly();
            entry.finish(-1);
            entry.sidecars.remove_all();
        }
        count
    }

    /// How many terminals are registered, exited-but-unreaped ones included.
    pub fn len(&self) -> usize {
        self.count()
    }

    /// Whether no terminal is registered.
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    /// Resolve a requested path (expanding a leading `~`) and make sure it stays
    /// inside the home directory.
    ///
    /// The terminal and the file browser must never disagree about what is
    /// inside the home directory, so the rule is [`crate::files::resolve_safe`]'s;
    /// only the error type is this module's.
    pub fn resolve_safe(&self, requested: &str) -> Result<String, TerminalError> {
        crate::files::resolve_safe(&self.paths, Some(requested))
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|_| TerminalError::Path)
    }

    /// Put an attached shim into the registry, returning it and the channel its
    /// link task will drain.
    ///
    /// Split from [`start_link`] so adoption can hold the link for one exchange
    /// — the repaint — before the task takes it over.
    fn register(
        self: &Arc<Self>,
        info: TerminalInfo,
        seq: u64,
        sidecars: Sidecars,
        shim_pid: u32,
        link: &Link,
        on_exit: Option<ExitHook>,
    ) -> (Arc<Entry>, mpsc::UnboundedReceiver<Frame>) {
        let id = info.id.clone();
        let (outbound, outbound_rx) = mpsc::unbounded_channel();
        let entry = Arc::new(Entry {
            seq,
            info: Mutex::new(info),
            replay: Mutex::new(Replay::default()),
            listeners: Mutex::new(Listeners::default()),
            outbound,
            // Live: the caller is holding the link this entry is being built
            // around, and hands it to the link task a moment from now.
            linked: watch::Sender::new(true),
            sidecars,
            shim_pid,
            shell_pid: AtomicU32::new(link.shell_pid),
            exited: AtomicBool::new(false),
            on_exit: Mutex::new(on_exit),
            cursor: AtomicU64::new(0),
            flushed: AtomicU64::new(0),
            detach: CancellationToken::new(),
            killed: AtomicBool::new(false),
            size_locked: AtomicBool::new(false),
        });
        // Registered before the link task starts, so a shell that exits
        // immediately still finds an entry to mark exited.
        self.entries.lock().expect("terminal registry poisoned").insert(id, entry.clone());
        (entry, outbound_rx)
    }

    fn count(&self) -> usize {
        self.entries.lock().expect("terminal registry poisoned").len()
    }

    fn drain(&self) -> Vec<Arc<Entry>> {
        self.entries
            .lock()
            .expect("terminal registry poisoned")
            .drain()
            .map(|(_, entry)| entry)
            .collect()
    }

    fn get(&self, id: &str) -> Option<Arc<Entry>> {
        self.entries.lock().expect("terminal registry poisoned").get(id).cloned()
    }

    /// The entry for `id`, unless its process is already gone.
    fn live(&self, id: &str) -> Option<Arc<Entry>> {
        self.get(id).filter(|entry| !entry.exited.load(Ordering::SeqCst))
    }
}

/// A client's handle on a terminal; dropping it detaches.
pub struct Attachment {
    /// Everything the terminal has produced so far, capped at [`BUFFER_CAP`].
    pub replay: String,
    entry: Arc<Entry>,
    listener_id: u64,
    /// Shared with this client's [`Sink`]: bytes queued but not yet taken.
    queued: Arc<AtomicUsize>,
    rx: mpsc::UnboundedReceiver<Chunk>,
}

impl Attachment {
    /// The next coalesced output chunk, or `None` once the terminal is finished.
    ///
    /// [`Chunk::TooSlow`] means this client's queue passed
    /// [`LISTENER_QUEUE_CAP`] and the socket should say so and close.
    pub async fn next_chunk(&mut self) -> Option<Chunk> {
        let chunk = self.rx.recv().await?;
        if let Chunk::Data(data) = &chunk {
            self.queued.fetch_sub(data.len(), Ordering::SeqCst);
        }
        Some(chunk)
    }

    /// Send keystrokes to the process, and say whether they will get there.
    ///
    /// `false` is the state this handle exists to stop hiding: the terminal is
    /// listed, the shell is alive, and the link in between is not — so the
    /// keystroke is queued against a reconnect that may never come. A caller
    /// that ignores the answer is a pane that looks perfectly normal and does
    /// nothing at all.
    pub fn write(&self, data: &str) -> bool {
        self.entry.tell(Frame::raw(proto::INPUT, data.as_bytes().to_vec()));
        self.entry.is_linked()
    }

    /// Whether the server can reach this terminal's shell at the moment.
    pub fn is_linked(&self) -> bool {
        self.entry.is_linked()
    }

    /// A handle that resolves whenever the link goes down or comes back.
    ///
    /// Handed out rather than awaited through `&mut self` because the socket
    /// selects on this and [`Attachment::next_chunk`] at once, and two mutable
    /// borrows of one `Attachment` cannot both live in a `select!`.
    pub fn link_changes(&self) -> watch::Receiver<bool> {
        self.entry.linked.subscribe()
    }

    /// Tell the kernel the window resized, ignoring implausible sizes — and any
    /// size at all on a terminal whose width the server pinned (see
    /// [`CreateSpec::size`]).
    pub fn resize(&self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 || cols >= 1000 || rows >= 1000 || self.entry.size_locked.load(Ordering::SeqCst) {
            return;
        }
        self.entry.tell(Frame::json(proto::RESIZE, &Resize { cols, rows }));
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        let mut listeners = self.entry.listeners.lock().expect("terminal listeners poisoned");
        listeners.sinks.retain(|sink| sink.id != self.listener_id);
    }
}

/// Spawn a shim, detached, and return its pid.
///
/// Every flag here is load-bearing. `process_group(0)` takes the shim out of the
/// server's process group, so a `kill` aimed at the server — or a shell that
/// signals the whole group on Ctrl-C — does not take the terminals with it.
/// `kill_on_drop(false)` keeps tokio from tidying it away when the handle goes.
/// Null stdio means the shim never holds a descriptor onto anything the server
/// owns; it speaks over its socket and nothing else.
async fn spawn_shim(sidecars: &Sidecars, program: &str, args: &[String], workdir: &str, env: &Env, size: Option<(u16, u16)>) -> std::io::Result<u32> {
    let mut command = tokio::process::Command::new(shim::shim_program());
    command
        .arg("pty-shim")
        .arg("--socket")
        .arg(sidecars.socket())
        .arg("--exit-file")
        .arg(sidecars.exit());
    // Before the `--`, so the shim opens the pty at this size rather than being
    // resized into it after the child has already read its width.
    if let Some((cols, rows)) = size {
        command.arg("--cols").arg(cols.to_string()).arg("--rows").arg(rows.to_string());
    }
    command.arg("--").arg(program).args(args);
    // A working directory that does not exist would fail the spawn outright, so
    // the shim inherits the server's own. The recorded `cwd` stays the requested
    // one either way.
    if Path::new(workdir).is_dir() {
        command.current_dir(workdir);
    }
    command.env_clear();
    command.envs(env);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        // stderr is kept, where the other two are not, because it is where a
        // panic goes. A shim that dies loudly is easy; one that loses a thread
        // and keeps running is the failure worth a log, and with stderr pointed
        // at `/dev/null` the message naming the line that panicked would be gone.
        // Best effort: a shim that cannot be given a log file is still worth
        // starting, so this falls back to discarding it.
        .stderr(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(sidecars.stderr())
                .map(std::process::Stdio::from)
                .unwrap_or_else(|_| std::process::Stdio::null()),
        )
        .kill_on_drop(false)
        .process_group(0);

    let mut child = command.spawn()?;
    let pid = child.id().unwrap_or(0);
    // Reaped explicitly rather than left to the orphan queue: while this server
    // lives the shim is its child, and an unreaped exit is a zombie for as long
    // as the server runs.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(pid)
}

/// `SIGTERM` a shim by pid.
///
/// Killing the shim is what actually ends a terminal whose shell will not go on
/// its own: an interactive login shell ignores `SIGTERM`, but it cannot ignore
/// its pty master being closed by a shim that is no longer there.
fn terminate_shim(pid: u32) {
    if pid <= 1 {
        return;
    }
    // SAFETY: a `SIGTERM` to a pid this process spawned, guarded above against
    // the two wildcard values.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
}

/// Connect to a shim that may not have finished binding yet.
async fn await_link(socket: &Path, cursor: u64) -> Result<Link, client::LinkError> {
    let deadline = Instant::now() + SPAWN_TIMEOUT;
    loop {
        match client::connect(socket, cursor).await {
            Ok(link) => return Ok(link),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }
}

/// Hand a link to its task.
fn start_link(
    registry: Weak<Registry>,
    entry: Arc<Entry>,
    link: Link,
    outbound: mpsc::UnboundedReceiver<Frame>,
    on_data: Option<DataHook>,
    carry: Vec<u8>,
) {
    let id = entry.sidecars.id().to_string();
    tokio::spawn(run_link(registry, id, entry, link, outbound, on_data, carry));
}

/// Read the identity of every scanned id, in the order the user's tabs are in.
///
/// A pass of its own, ahead of the adoption itself, because it is the cheap half
/// — one small file read each, no shim involved — and everything the concurrent
/// half needs decided up front depends on having seen all of them: the cap, and
/// the sequence a terminal created later has to sort after.
///
/// An id with no meta file is not a terminal at all. It is debris from a crash
/// between spawning a shim and recording what it was, and is swept here rather
/// than being handed to a handshake that has nothing to identify.
async fn identify(dir: &Path, ids: &[String]) -> Vec<(Meta, Sidecars)> {
    let mut candidates = Vec::with_capacity(ids.len());
    for id in ids {
        let sidecars = Sidecars::new(dir, id);
        let Some(meta) = sidecars.read_meta().await else {
            tracing::info!(%id, "sweeping pty-shim debris with no identity");
            sidecars.remove_all();
            continue;
        };
        candidates.push((meta, sidecars));
    }
    candidates.sort_by_key(|(meta, _)| meta.created_seq);
    candidates
}

/// Ask an adopted shim to repaint, and fold everything up to the reply into the
/// replay buffer.
///
/// The exchange is done here, synchronously, rather than left to the link task,
/// because the terminal is about to be listed: `GET /api/terminals` promising a
/// terminal whose pane is blank for another few hundred milliseconds is the
/// restart looking broken at exactly the moment it worked.
///
/// Bounded, though. A shim that will not answer must not stop the server from
/// listening; the terminal is still adopted and simply gets its screen from the
/// next output the shell produces.
async fn adopt_snapshot(entry: &Arc<Entry>, mut link: Link) -> (Link, Vec<u8>) {
    let mut carry = Vec::new();
    if link.send(&Frame::empty(proto::SNAPSHOT)).await.is_err() {
        return (link, carry);
    }
    let deadline = Instant::now() + SNAPSHOT_TIMEOUT;
    loop {
        let Ok(Some(frame)) = tokio::time::timeout_at(deadline, link.recv()).await else {
            tracing::warn!(id = entry.sidecars.id(), "a shim did not repaint in time; adopting it anyway");
            break;
        };
        // Anything before the repaint is backlog the shim owed us, and it is
        // folded in exactly as the link task would have folded it.
        let ingested = ingest(entry, frame, &mut carry, None);
        if matches!(ingested, Ingested::Repaint | Ingested::Exited(_)) {
            break;
        }
    }
    entry.flush();
    (link, carry)
}

/// One terminal's link: frames in, frames out, and the reconnect in between.
#[allow(clippy::too_many_arguments)]
async fn run_link(
    registry: Weak<Registry>,
    id: String,
    entry: Arc<Entry>,
    mut link: Link,
    mut outbound: mpsc::UnboundedReceiver<Frame>,
    on_data: Option<DataHook>,
    mut carry: Vec<u8>,
) {
    let mut flush_at: Option<Instant> = None;
    // When output last went out: a chunk after a quiet interval is flushed at
    // once, a chunk within one waits for the interval's end.
    let mut flushed_at: Option<Instant> = None;

    let exit_code = 'linked: loop {
        loop {
            tokio::select! {
                biased;
                () = entry.detach.cancelled() => {
                    entry.cut();
                    return;
                }
                frame = link.recv() => match frame {
                    Some(frame) => match ingest(&entry, frame, &mut carry, on_data.as_ref()) {
                        Ingested::Nothing => {}
                        Ingested::Data => {
                            let now = Instant::now();
                            if entry.replay.lock().expect("terminal replay poisoned").pending.len() >= FLUSH_CAP {
                                entry.flush();
                                flush_at = None;
                                flushed_at = Some(now);
                                // A burst must not go out in one go. Frames
                                // that piled up while this task waited for the
                                // CPU are all ready at once, and one poll may
                                // ingest a hundred of them at up to 256 KiB
                                // each. That put more than `LISTENER_QUEUE_CAP`
                                // into every client's queue before any socket
                                // task had run, and cut off a client that was
                                // keeping up perfectly well. Yielding after each
                                // full flush gives every socket a turn, so the
                                // cap measures the client, not the scheduler.
                                tokio::task::yield_now().await;
                            } else if flush_at.is_none() {
                                match flushed_at {
                                    Some(last) if now < last + FLUSH_INTERVAL => flush_at = Some(last + FLUSH_INTERVAL),
                                    _ => {
                                        entry.flush();
                                        flushed_at = Some(now);
                                    }
                                }
                            }
                        }
                        // A repaint is the whole point of reattaching: it goes
                        // out now rather than in 16 ms.
                        Ingested::Repaint => {
                            entry.flush();
                            flush_at = None;
                        }
                        Ingested::Exited(code) => break 'linked code,
                    },
                    // The shim went away. It may be restarting nothing at all —
                    // a shim does not restart — so this is either a socket that
                    // dropped under us or a shell that died while we were not
                    // looking. `relink` decides which.
                    None => break,
                },
                Some(frame) = outbound.recv() => {
                    if link.send(&frame).await.is_err() {
                        break;
                    }
                }
                () = deadline(flush_at) => {
                    entry.flush();
                    flush_at = None;
                    flushed_at = Some(Instant::now());
                }
            }
        }

        // Out of the inner loop means there is no link and, until there is one
        // again, nobody draining `outbound`. Say so *before* the backoff rather
        // than after it: those seconds are the whole window in which a user
        // types into a terminal that cannot hear them.
        entry.set_linked(false);
        entry.flush();
        match relink(&entry).await {
            Relinked::Attached(fresh) => {
                link = fresh;
                entry.set_linked(true);
            }
            Relinked::Exited(code) => break 'linked code,
            Relinked::Detached => {
                entry.cut();
                return;
            }
        }
    };

    // Down for good, and for a different reason than a reconnect: the entry
    // lingers for [`REAP_AFTER`] so attached clients read the exit note, and it
    // must not spend that time claiming a link it will never have again.
    entry.set_linked(false);
    entry.flush();
    entry.note_exit();
    entry.finish(exit_code);
    // The shell is gone for good, so nothing here is adoptable any more.
    entry.sidecars.remove_all();

    // Keep the entry around briefly so attached clients see the exit note.
    tokio::time::sleep(REAP_AFTER).await;
    if let Some(registry) = registry.upgrade() {
        registry.entries.lock().expect("terminal registry poisoned").remove(&id);
    }
}

/// What one inbound frame did.
enum Ingested {
    /// Nothing the browser cares about.
    Nothing,
    /// Output was buffered.
    Data,
    /// A screen repaint was buffered, and should go out immediately.
    Repaint,
    /// The shell exited.
    Exited(i32),
}

/// Fold one frame from the shim into the browser-facing stream.
fn ingest(entry: &Entry, frame: Frame, carry: &mut Vec<u8>, on_data: Option<&DataHook>) -> Ingested {
    match frame.kind {
        proto::OUTPUT => {
            entry.advance(frame.payload.len() as u64);
            carry.extend_from_slice(&frame.payload);
            let text = take_utf8(carry);
            if text.is_empty() {
                return Ingested::Nothing;
            }
            // Scanners see the raw stream: coalescing is a transport concern,
            // and a token could otherwise be split across two flushes for good.
            if let Some(hook) = on_data {
                hook(&text);
            }
            entry.push_pending(&text);
            Ingested::Data
        }
        proto::GAP => {
            let Some(gap) = frame.parse::<proto::Gap>() else {
                return Ingested::Nothing;
            };
            // A partial character from before the gap cannot be completed by
            // anything after it.
            carry.clear();
            entry.set_cursor(gap.to);
            let note = gap_note(gap.lost());
            tracing::warn!(lost = gap.lost(), "a terminal lost output to its shim's ring");
            if let Some(hook) = on_data {
                hook(&note);
            }
            entry.push_pending(&note);
            Ingested::Repaint
        }
        proto::SNAPSHOT => {
            // A repaint is self-contained escape sequences, not a continuation
            // of the output stream, so it neither uses the carry nor advances
            // the cursor — the shim already sent every byte it reflects.
            carry.clear();
            let text = String::from_utf8_lossy(&frame.payload).into_owned();
            if text.is_empty() {
                return Ingested::Nothing;
            }
            entry.push_pending(&text);
            Ingested::Repaint
        }
        proto::EXIT => Ingested::Exited(frame.parse::<proto::Exit>().map(|exit| exit.code).unwrap_or(-1)),
        _ => Ingested::Nothing,
    }
}

/// What came of trying to get the link back.
enum Relinked {
    /// Reattached, with a repaint already requested.
    Attached(Link),
    /// The shell is gone; this is its status.
    Exited(i32),
    /// The server is shutting down.
    Detached,
}

/// Reconnect to a shim, backing off, until it answers or is proven gone.
///
/// The two exits that are not "reconnected" are what keep this from spinning
/// forever on a shim that will never come back: a tombstone is the shell saying
/// how it ended, and a dead pid is the shim having been killed without getting
/// the chance to. Everything else is a socket that may yet come back — a machine
/// under load, a filesystem that stalled — and is worth waiting for, because on
/// the other side of it is a live shell.
async fn relink(entry: &Arc<Entry>) -> Relinked {
    let mut delay = RECONNECT_MIN;
    loop {
        if entry.detach.is_cancelled() {
            return Relinked::Detached;
        }
        if let Some(tombstone) = entry.sidecars.read_tombstone().await {
            return Relinked::Exited(tombstone.exit_code);
        }
        let shell = entry.shell_pid.load(Ordering::SeqCst);
        if shell > 1 && !crate::procs::alive(shell) {
            tracing::warn!(shell_pid = shell, "a terminal's shell is gone and its shim left no exit file");
            return Relinked::Exited(-1);
        }

        match client::connect(&entry.sidecars.socket(), entry.cursor.load(Ordering::SeqCst)).await {
            Ok(link) => {
                entry.shell_pid.store(link.shell_pid, Ordering::SeqCst);
                entry.tell(Frame::empty(proto::SNAPSHOT));
                return Relinked::Attached(link);
            }
            Err(error) => tracing::debug!(%error, "retrying a pty shim link"),
        }

        tokio::select! {
            () = entry.detach.cancelled() => return Relinked::Detached,
            () = tokio::time::sleep(delay) => {}
        }
        delay = (delay * 2).min(RECONNECT_MAX);
    }
}

/// Sweep a killed terminal's sidecars once its shim has confirmed the death.
///
/// The `k` frame is the polite path and usually all it takes. The fallback is
/// for the case the frame could not be delivered at all — a link that was
/// reconnecting when the kill arrived — where the pids the hello handed over are
/// the only thing left to act on.
async fn reap_killed(registry: Weak<Registry>, entry: Arc<Entry>) {
    let deadline = Instant::now() + TOMBSTONE_GRACE;
    while Instant::now() < deadline && !entry.sidecars.tombstoned() {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    if !entry.sidecars.tombstoned() {
        tracing::warn!(id = entry.sidecars.id(), "a killed terminal left no exit file; signalling it directly");
        entry.signal_directly();
    }
    // The tombstone is the shim's first act on the way out, not its last: the
    // `x` frame follows it once the shim's reader has drained the pty
    // (`pty_shim::child::run`, steps 1 and 2). Cancelling `detach` as soon as
    // the tombstone appears would beat that frame on a busy machine, and the
    // link task, which checks `detach` first, would stop without noting the
    // exit. So the link task is given time to carry the frame itself and is
    // only cut when it has not, and a cut on a killed terminal still notes
    // the exit (`Entry::cut`).
    let deadline = Instant::now() + EXIT_FRAME_GRACE;
    while Instant::now() < deadline && !entry.exited.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    entry.detach.cancel();
    entry.sidecars.remove_all();
    if let Some(registry) = registry.upgrade() {
        let mut dying = registry.dying.lock().expect("terminal registry poisoned");
        dying.retain(|other| !Arc::ptr_eq(other, &entry));
    }
}

/// Resolves at `deadline`, or never when there is none.
async fn deadline(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Take every complete character out of `carry`, leaving a partial multi-byte
/// sequence behind for the next read.
///
/// A pty read can land anywhere, including the middle of a UTF-8 sequence, and
/// the frames going out are text — so a naive `from_utf8_lossy` per read would
/// permanently corrupt any character unlucky enough to straddle two reads.
fn take_utf8(carry: &mut Vec<u8>) -> String {
    let mut out = String::new();
    loop {
        match std::str::from_utf8(carry) {
            Ok(text) => {
                out.push_str(text);
                carry.clear();
                return out;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                out.push_str(std::str::from_utf8(&carry[..valid]).expect("valid_up_to is a char boundary"));
                match error.error_len() {
                    // Truncated, not invalid: keep the tail for the next read.
                    None => {
                        carry.drain(..valid);
                        return out;
                    }
                    Some(len) => {
                        out.push('\u{fffd}');
                        carry.drain(..valid + len);
                    }
                }
            }
        }
    }
}

/// Keep the last `cap` bytes of `text`, cutting on a character boundary.
///
/// This slices bytes, so a buffer holding wide characters keeps somewhat fewer
/// than `cap` of them. The cap is a memory bound, not part of the contract.
fn truncate_front(text: &mut String, cap: usize) {
    if text.len() <= cap {
        return;
    }
    let mut cut = text.len() - cap;
    while cut < text.len() && !text.is_char_boundary(cut) {
        cut += 1;
    }
    text.drain(..cut);
}

/// `terminal N`, unless the caller named it.
fn default_title(title: Option<&str>, ordinal: usize) -> String {
    match title.map(str::trim) {
        Some(title) if !title.is_empty() => title.to_string(),
        _ => format!("terminal {ordinal}"),
    }
}

/// The user's login shell, falling back to zsh and then bash when `SHELL` is
/// unset or empty.
fn login_shell() -> String {
    match std::env::var("SHELL") {
        Ok(shell) if !shell.is_empty() => shell,
        _ if Path::new("/bin/zsh").exists() => "/bin/zsh".to_string(),
        _ => "/bin/bash".to_string(),
    }
}

/// The scrubbed environment every terminal starts with: [`crate::process_env`]'s,
/// plus the `TERM` a pty needs and a spawned CLI does not.
fn terminal_env() -> Env {
    process_env::clean_env([("TERM", TERM)])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A registry on a throwaway home and instance.
    fn registry(home: &Path, instance: &str) -> Arc<Registry> {
        let paths = Arc::new(Paths::new(home.to_path_buf()));
        let dirs = InstanceDirs::new(&paths.data_dir(), instance).expect("instance name");
        dirs.create().expect("create instance dirs");
        Registry::new(paths, Arc::new(dirs))
    }

    #[test]
    fn partial_characters_survive_a_split_read() {
        // "é" is two bytes; feed them one read apart.
        let mut carry = vec![0xc3];
        assert_eq!(take_utf8(&mut carry), "");
        carry.push(0xa9);
        assert_eq!(take_utf8(&mut carry), "é");
        assert!(carry.is_empty());

        // Genuinely invalid bytes become one replacement character each.
        let mut carry = vec![b'a', 0xff, b'b'];
        assert_eq!(take_utf8(&mut carry), "a\u{fffd}b");
    }

    #[test]
    fn the_replay_buffer_keeps_its_tail() {
        let mut text = "abcdef".to_string();
        truncate_front(&mut text, 4);
        assert_eq!(text, "cdef");

        // "aéb" is four bytes; a cap of two lands inside the two-byte "é".
        let mut text = "aéb".to_string();
        truncate_front(&mut text, 2);
        assert_eq!(text, "b", "a boundary-crossing cut moves forward, it does not corrupt");

        // A cap that happens to land on a boundary keeps everything after it.
        let mut text = "aéb".to_string();
        truncate_front(&mut text, 3);
        assert_eq!(text, "éb");
    }

    #[test]
    fn the_harness_variables_are_stripped() {
        std::env::set_var("CLAUDECODE", "1");
        std::env::set_var("CLAUDE_CONFIG_DIR", "/nope");
        std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-nope");
        std::env::set_var("RH_TERMINAL_KEEP", "kept");

        let env = terminal_env();
        assert!(!env.contains_key("CLAUDECODE"));
        assert!(!env.contains_key("CLAUDE_CONFIG_DIR"));
        assert!(!env.contains_key("ANTHROPIC_API_KEY"));
        assert_eq!(env.get("RH_TERMINAL_KEEP").map(String::as_str), Some("kept"));
        assert_eq!(env.get("TERM").map(String::as_str), Some(TERM));
    }

    /// A tool the first-run guide found after the server started is on the
    /// `PATH` of the next terminal, with no restart (`crate::setup`).
    #[test]
    fn a_new_terminal_gets_the_children_s_path() {
        let _guard = crate::setup::tests::ChildPathGuard::take();
        let dir = tempfile::tempdir().expect("tempdir");
        // A superset of what was there, so a test running beside this one
        // still finds everything it would have.
        let path = format!("{}:{}", crate::process_env::child_path(), dir.path().display());
        crate::process_env::set_child_path(Some(path.clone()));
        assert_eq!(terminal_env().get("PATH"), Some(&path));
    }

    #[test]
    fn a_working_directory_may_not_escape_home() {
        let home = tempfile::tempdir().expect("tempdir");
        let registry = registry(home.path(), "cwd");
        let home_str = home.path().to_string_lossy().into_owned();

        assert_eq!(registry.resolve_safe("").expect("home"), home_str);
        assert_eq!(registry.resolve_safe("~").expect("tilde"), home_str);
        assert_eq!(registry.resolve_safe("~/work").expect("tilde path"), format!("{home_str}/work"));
        assert_eq!(
            registry.resolve_safe(&format!("{home_str}/a/../b")).expect("dotdot"),
            format!("{home_str}/b")
        );
        assert!(registry.resolve_safe("/etc").is_err());
        assert!(registry.resolve_safe(&format!("{home_str}/../elsewhere")).is_err());
    }

    /// An entry with no shim behind it, for the bookkeeping that does not need
    /// one. Everything that *does* is an integration test, because a shim is a
    /// second process and the unit-test binary is not the server.
    ///
    /// The outbound receiver comes back with it and must be kept: it is what a
    /// link task would be draining, and dropping it turns every [`Entry::tell`]
    /// into a discarded `send` — the very failure these tests are about.
    fn bare_entry(dir: &Path) -> (Arc<Entry>, mpsc::UnboundedReceiver<Frame>) {
        let (outbound, outbound_rx) = mpsc::unbounded_channel();
        let entry = Arc::new(Entry {
            seq: 0,
            info: Mutex::new(TerminalInfo {
                id: "unit".into(),
                title: "terminal 1".into(),
                cwd: dir.to_string_lossy().into_owned(),
                created_at: "2026-01-01T00:00:00.000Z".into(),
            }),
            replay: Mutex::new(Replay::default()),
            listeners: Mutex::new(Listeners::default()),
            outbound,
            linked: watch::Sender::new(true),
            sidecars: Sidecars::new(dir, "unit"),
            shim_pid: 0,
            shell_pid: AtomicU32::new(0),
            exited: AtomicBool::new(false),
            on_exit: Mutex::new(None),
            cursor: AtomicU64::new(0),
            flushed: AtomicU64::new(0),
            detach: CancellationToken::new(),
            killed: AtomicBool::new(false),
            size_locked: AtomicBool::new(false),
        });
        (entry, outbound_rx)
    }

    /// The cursor is what a restart resumes from, so how often it is written is
    /// the difference between duplicated output and lost output. It lags by up
    /// to [`shim::CURSOR_FLUSH_EVERY`], deliberately, and never runs ahead.
    #[tokio::test]
    async fn the_cursor_file_lags_by_one_flush_interval_and_never_leads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (entry, _outbound) = bare_entry(dir.path());

        // Nothing is written until a whole interval has been consumed.
        entry.advance(shim::CURSOR_FLUSH_EVERY - 1);
        assert_eq!(entry.sidecars.read_cursor().await, 0, "a partial interval is not recorded");

        entry.advance(1);
        assert_eq!(entry.sidecars.read_cursor().await, shim::CURSOR_FLUSH_EVERY);

        // And the record never claims more than has actually been consumed.
        entry.advance(shim::CURSOR_FLUSH_EVERY / 2);
        let recorded = entry.sidecars.read_cursor().await;
        assert_eq!(recorded, shim::CURSOR_FLUSH_EVERY, "still within the same interval");
        assert!(recorded < entry.cursor.load(Ordering::SeqCst), "the cursor file lags rather than leads");

        entry.advance(shim::CURSOR_FLUSH_EVERY);
        assert_eq!(entry.sidecars.read_cursor().await, entry.cursor.load(Ordering::SeqCst));
    }

    /// A gap is the one thing that moves the cursor without any output being
    /// consumed, and it is written out immediately: what was lost must not be
    /// asked for again after a restart.
    #[tokio::test]
    async fn a_gap_jumps_the_cursor_and_records_it_at_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (entry, _outbound) = bare_entry(dir.path());
        let mut carry = Vec::new();

        entry.advance(10);
        let frame = Frame::json(proto::GAP, &proto::Gap { from: 10, to: 4106 });
        assert!(matches!(ingest(&entry, frame, &mut carry, None), Ingested::Repaint));

        assert_eq!(entry.cursor.load(Ordering::SeqCst), 4106);
        assert_eq!(
            entry.sidecars.read_cursor().await,
            4106,
            "a gap is recorded without waiting for a flush interval"
        );
        entry.flush();
        let replay = entry.replay.lock().expect("replay");
        assert!(
            replay.buffer.contains("output gap: 4096 bytes lost"),
            "the loss is visible: {:?}",
            replay.buffer
        );
    }

    /// A repaint is not part of the output stream: it must not move the cursor,
    /// or the bytes it stands in for would never be asked for again.
    #[tokio::test]
    async fn a_snapshot_repaints_without_consuming_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (entry, _outbound) = bare_entry(dir.path());
        let mut carry = Vec::new();

        let frame = Frame::raw(proto::OUTPUT, b"hello".to_vec());
        assert!(matches!(ingest(&entry, frame, &mut carry, None), Ingested::Data));
        assert_eq!(entry.cursor.load(Ordering::SeqCst), 5);

        let frame = Frame::raw(proto::SNAPSHOT, b"\x1b[H\x1b[Jhello".to_vec());
        assert!(matches!(ingest(&entry, frame, &mut carry, None), Ingested::Repaint));
        assert_eq!(entry.cursor.load(Ordering::SeqCst), 5, "a repaint is not output");
    }

    /// A terminal whose link is down is indistinguishable from a healthy one at
    /// the sending end, because an unbounded channel accepts a keystroke whether
    /// or not anything will ever drain it. The link flag is the only thing that
    /// can tell a client apart.
    #[tokio::test]
    async fn a_link_that_goes_down_stops_pretending_keystrokes_arrive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (entry, mut outbound) = bare_entry(dir.path());
        let attachment = entry.attach();

        assert!(attachment.is_linked(), "an entry is born holding a link");
        assert!(attachment.write("ls\r"), "a keystroke on a live link is delivered");
        assert!(outbound.try_recv().is_ok(), "and really was queued for the shim");

        // What `run_link` does the moment it falls out of its inner loop, before
        // it starts backing off inside `relink`.
        entry.set_linked(false);
        assert!(!attachment.is_linked());
        assert!(!attachment.write("ls\r"), "a keystroke into a dead link is reported, not swallowed");
        assert!(
            outbound.try_recv().is_ok(),
            "it is still queued, because a reconnect a second later should deliver it"
        );

        entry.set_linked(true);
        assert!(attachment.write("ls\r"), "and a relink makes the terminal usable again");
    }

    /// A client told the terminal is unreachable has to hear when it stops being,
    /// or the note becomes a lie that outlives the problem it described.
    #[tokio::test]
    async fn a_relink_wakes_the_clients_that_were_told_the_link_was_down() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (entry, _outbound) = bare_entry(dir.path());
        let attachment = entry.attach();
        let mut changes = attachment.link_changes();

        entry.set_linked(false);
        changes.changed().await.expect("the entry outlives its attachment");
        assert!(!*changes.borrow_and_update());

        // A failed retry inside `relink` is not news: a terminal that was down
        // and is still down must not blink at the client once a second.
        entry.set_linked(false);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), changes.changed()).await.is_err(),
            "a link that was already down wakes nobody"
        );

        entry.set_linked(true);
        changes.changed().await.expect("the entry outlives its attachment");
        assert!(*changes.borrow_and_update(), "the client learns the terminal is usable again");
    }

    /// An attachment's replay followed by everything already on its queue.
    fn everything_sent(attachment: &mut Attachment) -> String {
        let mut seen = attachment.replay.clone();
        while let Ok(chunk) = attachment.rx.try_recv() {
            match chunk {
                Chunk::Data(data) => seen.push_str(&data),
                Chunk::TooSlow => panic!("a client this small is never too slow"),
            }
        }
        seen
    }

    /// A client that attaches while output waits out the coalescing interval
    /// gets each byte exactly once: the pending output is flushed into the
    /// replay at attach, and the flush at the end of the interval must not send
    /// it a second time.
    #[tokio::test]
    async fn an_attach_mid_interval_gets_each_byte_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (entry, _outbound) = bare_entry(dir.path());

        entry.push_pending("one ");
        entry.flush();
        // Held back: within an interval of the last flush.
        entry.push_pending("two ");
        let mut attachment = entry.attach();
        // The interval ends.
        entry.flush();
        entry.push_pending("three");
        entry.flush();

        let seen = everything_sent(&mut attachment);
        assert_eq!(seen, "one two three", "the replay and the queue together hold the stream once");
    }

    /// The other half of the same race, which only a second thread can reach:
    /// a flush that landed between an attach's read of the replay and its
    /// registration went to every client but that one, and was lost to it.
    #[test]
    fn attaches_racing_the_flushes_see_the_stream_whole() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (entry, _outbound) = bare_entry(dir.path());
        let producer = {
            let entry = entry.clone();
            std::thread::spawn(move || {
                // About 24 KiB in all, inside BUFFER_CAP, so the replay never
                // drops its front and every client must end up with all of it.
                for n in 0..5_000 {
                    entry.push_pending(&format!("{n},"));
                    if n % 3 == 0 {
                        entry.flush();
                    }
                }
                entry.flush();
            })
        };
        let mut attachments = Vec::new();
        while !producer.is_finished() && attachments.len() < 500 {
            attachments.push(entry.attach());
            std::thread::yield_now();
        }
        producer.join().expect("the producer");

        let whole = entry.replay.lock().expect("replay").buffer.clone();
        for (n, mut attachment) in attachments.into_iter().enumerate() {
            let seen = everything_sent(&mut attachment);
            assert!(seen == whole, "client {n} saw {} bytes of a {}-byte stream", seen.len(), whole.len());
        }
    }

    /// A burst of output that is all there at once must not cut off a client
    /// that takes every chunk as soon as it gets the chance. Here it is 6 MiB
    /// already queued on the link, in the largest frames a shim sends, with
    /// the client on the same thread as the link task, which is where it
    /// stood when its process had been waiting for the CPU.
    #[tokio::test]
    async fn a_burst_does_not_cut_off_a_client_that_keeps_up() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (entry, outbound) = bare_entry(dir.path());
        let mut attachment = entry.attach();
        const BURST: usize = 6 * 1024 * 1024;
        let frames = (0..BURST / (256 * 1024)).map(|_| Frame::raw(proto::OUTPUT, vec![b'y'; 256 * 1024])).collect();
        let (link, _queued) = client::Link::preloaded(frames).expect("a preloaded link");
        tokio::spawn(run_link(Weak::new(), "unit".into(), entry.clone(), link, outbound, None, Vec::new()));

        let mut seen = 0;
        while seen < BURST {
            match tokio::time::timeout(Duration::from_secs(5), attachment.next_chunk()).await.expect("the burst arrives") {
                Some(Chunk::Data(data)) => seen += data.len(),
                Some(Chunk::TooSlow) => panic!("a client taking every chunk as it came was cut off after {seen} bytes"),
                None => panic!("the stream ended after {seen} bytes"),
            }
        }
        entry.detach.cancel();
    }

    /// How a [`stand_in_shim`]'s shell ends.
    enum ShellEnd {
        /// On a `k` frame, the way a real shim ends it: tombstone first and `x`
        /// frame second, but with this long between the two. That gap is what a
        /// busy machine produces, here with a fixed length. `None` never sends
        /// the frame at all.
        Killed(Option<Duration>),
        /// On its own, right after the hello: tombstone, last output, `x`.
        Exits,
    }

    /// A stand-in shim on `sidecars`' socket, minding a shell it says is
    /// `shell_pid` and that ends as `end` says.
    ///
    /// Nothing here signals anything. Pass pid 0 when no real process should
    /// be reachable through the entry at all.
    fn stand_in_shim(sidecars: &Sidecars, shell_pid: u32, end: ShellEnd) {
        sidecars.ensure_socket_dir().expect("the shim socket directory");
        let listener = tokio::net::UnixListener::bind(sidecars.socket()).expect("bind the shim socket");
        let tombstone = sidecars.exit();
        let bury = move || {
            let record = proto::Tombstone {
                exit_code: -1,
                at: "2026-01-01T00:00:00.000Z".into(),
            };
            std::fs::write(&tombstone, serde_json::to_vec(&record).expect("encode")).expect("write the tombstone");
        };
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("the server dials in");
            let (mut reader, mut writer) = stream.into_split();
            let hello = proto::read_frame(&mut reader).await.expect("read the hello").expect("a hello");
            assert_eq!(hello.kind, proto::HELLO);
            let reply = Frame::json(proto::HELLO, &proto::Hello2 { shell_pid, total: 0 });
            proto::write_frame(&mut writer, &reply).await.expect("answer the hello");
            let announce = match end {
                ShellEnd::Exits => {
                    bury();
                    Duration::ZERO
                }
                ShellEnd::Killed(announce) => {
                    loop {
                        match proto::read_frame(&mut reader).await {
                            Ok(Some(frame)) if frame.kind == proto::KILL => break,
                            Ok(Some(_)) => continue,
                            _ => return,
                        }
                    }
                    bury();
                    let Some(delay) = announce else {
                        // The link stays open and nothing more is said.
                        std::future::pending::<()>().await;
                        return;
                    };
                    delay
                }
            };
            tokio::time::sleep(announce).await;
            let _ = proto::write_frame(&mut writer, &Frame::raw(proto::OUTPUT, b"bye\r\n".to_vec())).await;
            let _ = proto::write_frame(&mut writer, &Frame::json(proto::EXIT, &proto::Exit { code: -1 })).await;
        });
    }

    /// A terminal on a [`stand_in_shim`] that ends as `end` says, with one client
    /// attached. Neither of its pids reaches a real process.
    async fn terminal_with_a_client(home: &Path, end: ShellEnd) -> (Arc<Registry>, String, Attachment) {
        terminal_with_pids(home, 0, 0, end).await
    }

    /// [`terminal_with_a_client`], with the shell and shim pids the entry will
    /// believe in.
    async fn terminal_with_pids(home: &Path, shell_pid: u32, shim_pid: u32, end: ShellEnd) -> (Arc<Registry>, String, Attachment) {
        let registry = registry(home, "kill");
        // A fresh id each time: the socket is named after it, and may sit in a
        // directory every test in this process shares.
        let id = uuid::Uuid::new_v4().to_string();
        let sidecars = Sidecars::new(registry.instance.pty_shims(), &id);
        stand_in_shim(&sidecars, shell_pid, end);
        let link = client::connect(&sidecars.socket(), 0).await.expect("link to the stand-in shim");
        let info = TerminalInfo {
            id: id.clone(),
            title: "terminal 1".into(),
            cwd: home.to_string_lossy().into_owned(),
            created_at: "2026-01-01T00:00:00.000Z".into(),
        };
        let (entry, outbound) = registry.register(info, 0, sidecars, shim_pid, &link, None);
        start_link(Arc::downgrade(&registry), entry, link, outbound, None, Vec::new());
        let attachment = registry.attach(&id).expect("attachable");
        (registry, id, attachment)
    }

    /// Everything a client is sent up to and including the exit note.
    async fn read_through_the_exit_note(attachment: &mut Attachment, within: Duration) -> String {
        let deadline = Instant::now() + within;
        let mut seen = String::new();
        while !seen.contains(EXIT_NOTE) {
            match tokio::time::timeout_at(deadline, attachment.next_chunk()).await {
                Ok(Some(Chunk::Data(data))) => seen.push_str(&data),
                Ok(other) => panic!("the stream ended without the exit note ({other:?}); saw {seen:?}"),
                Err(_) => panic!("no exit note within {within:?}; saw {seen:?}"),
            }
        }
        seen
    }

    /// The shim writes its tombstone, then drains the pty, then sends `x`. A
    /// reaper that cuts the link on the tombstone would leave without noting the
    /// exit, and the client would wait for a note that never comes; here that
    /// gap is a fixed 300 ms, well past the reaper's 25 ms poll.
    #[tokio::test]
    async fn a_kill_waits_for_the_exit_frame_that_trails_the_tombstone() {
        let home = tempfile::tempdir().expect("tempdir");
        let (registry, id, mut attachment) = terminal_with_a_client(home.path(), ShellEnd::Killed(Some(Duration::from_millis(300)))).await;

        assert!(registry.kill_terminal(&id));
        let seen = read_through_the_exit_note(&mut attachment, Duration::from_secs(5)).await;
        assert_eq!(
            seen,
            format!("bye\r\n{EXIT_NOTE}"),
            "the shell's last output reaches the client, and then the note"
        );
    }

    /// A shim that never sends `x` cannot keep the note from a client either:
    /// when the reaper stops waiting and cuts the link, the cut notes the exit.
    #[tokio::test]
    async fn a_killed_terminal_whose_shim_never_announces_still_tells_its_clients() {
        let home = tempfile::tempdir().expect("tempdir");
        let (registry, id, mut attachment) = terminal_with_a_client(home.path(), ShellEnd::Killed(None)).await;

        assert!(registry.kill_terminal(&id));
        let seen = read_through_the_exit_note(&mut attachment, EXIT_FRAME_GRACE + Duration::from_secs(5)).await;
        assert_eq!(seen, EXIT_NOTE);
    }

    /// A detach on shutdown must not note an exit: the shell is still running,
    /// and the next server adopts it.
    #[tokio::test]
    async fn a_shutdown_detach_tells_clients_nothing() {
        let home = tempfile::tempdir().expect("tempdir");
        let (registry, _id, mut attachment) = terminal_with_a_client(home.path(), ShellEnd::Killed(None)).await;

        assert_eq!(registry.shutdown_all(), 1);
        assert!(
            tokio::time::timeout(Duration::from_millis(300), attachment.next_chunk()).await.is_err(),
            "no exit note for a shell that lives on"
        );
    }

    /// A process this test owns, in a process group of its own, standing in
    /// for whatever reused a dead terminal's pid.
    fn bystander() -> std::process::Child {
        use std::os::unix::process::CommandExt;
        std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .expect("spawn a bystander")
    }

    /// Whether a bystander is still running, allowing a signal sent a moment
    /// ago the time to land.
    fn untouched(child: &mut std::process::Child) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_millis(300);
        while std::time::Instant::now() < deadline {
            if child.try_wait().expect("try_wait").is_some() {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    /// Closing a terminal that has already exited, within the [`REAP_AFTER`]
    /// window its clients get to read the note in, must start no reaper: the
    /// reaper would wait for a tombstone the link task has already swept, then
    /// `SIGKILL` the dead shell's process group and `SIGTERM` the shim pid,
    /// either of which may by then belong to an unrelated process. Here both
    /// pids name live bystanders, and [`Entry::signal_directly`] — which is what
    /// [`Registry::kill_all`] uses — makes any signal show at once.
    #[tokio::test]
    async fn closing_a_terminal_that_already_exited_signals_nobody() {
        let home = tempfile::tempdir().expect("tempdir");
        let mut shell = bystander();
        let mut shim = bystander();
        let (registry, id, mut attachment) = terminal_with_pids(home.path(), shell.id(), shim.id(), ShellEnd::Exits).await;
        read_through_the_exit_note(&mut attachment, Duration::from_secs(5)).await;

        assert!(registry.kill_terminal(&id), "a finished tab that is still listed for its note closes");
        assert!(!registry.kill_terminal(&id), "and is gone once it has");
        let reaping = !registry.dying.lock().expect("dying").is_empty();
        registry.kill_all();

        let (shell_ok, shim_ok) = (untouched(&mut shell), untouched(&mut shim));
        for child in [&mut shell, &mut shim] {
            let _ = child.kill();
            let _ = child.wait();
        }
        assert!(shell_ok, "the dead shell's process group was signalled");
        assert!(shim_ok, "the dead terminal's shim pid was signalled");
        assert!(!reaping, "no reaper for a process that is already gone");
    }

    #[test]
    fn the_default_title_counts_terminals() {
        assert_eq!(default_title(None, 3), "terminal 3");
        assert_eq!(default_title(Some("   "), 3), "terminal 3");
        assert_eq!(default_title(Some("  build  "), 3), "build");
    }
}
