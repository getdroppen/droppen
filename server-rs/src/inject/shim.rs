//! Claude processes that outlive the server.
//!
//! [`super`] drives a CLI the obvious way: spawn `claude --print`, hold its
//! pipes, read until it exits. That works for a run that finishes before the
//! server does, and not otherwise — a pipe dies with whoever holds it, so every
//! self-upgrade takes every in-flight turn with it. A *resident* claude, one
//! process per session fed prompt after prompt over `--input-format
//! stream-json`, cannot be held that way at all: its whole point is to still be
//! there tomorrow.
//!
//! So it is held one process out, exactly as [`crate::pty_shim`] holds a shell:
//!
//! ```text
//!   server  ──unix socket──▶  session shim  ──pipes──▶  claude
//!   (restarts)                (does not)                (never notices)
//! ```
//!
//! The shim is this same binary under a hidden `session-shim` subcommand,
//! spawned detached and in its own process group. It owns claude's three pipes
//! and nothing else: bytes in one side, bytes out the other, no idea what
//! stream-json is. Deliberately so — the shim's version is pinned by whatever
//! spawned it, so a shim that needs changing is a session that has to die, and
//! the smallest thing that can work is the thing least likely to need changing.
//!
//! # The wire
//!
//! ```text
//!   client → shim, first line:  {"cursor":<stdout bytes already received>}
//!   shim → client, first line:  {"shim_hello":{"claude_pid":N,"total":T}}
//!   then: claude's stdout from that offset on, verbatim, forever.
//! ```
//!
//! Everything the client sends after its first line is claude's stdin, verbatim
//! — with one exception, [`CLOSE_STDIN`], which is how a caller asks for the
//! clean shutdown that a pipe close means and a disconnect must not.
//!
//! The cursor is in the protocol and [`attach`] always sends `0`. That is not an
//! oversight: resuming at a remembered offset trades a correctness anchor for
//! bandwidth nobody is short of, and the anchor — replay everything, let the
//! caller drop what it has already seen by message id — is what makes a
//! reattach after an unclean kill safe. The field stays because removing it
//! would be a protocol change later.
//!
//! # What a disconnected client misses
//!
//! Nothing, until it does. While no client is attached, claude's stdout goes
//! into a bounded ring ([`DEFAULT_BUFFER_BYTES`]) and a reattach replays it. A
//! ring that overflows drops **whole lines** and says so with a
//! `{"shim_gap":{"from":A,"to":B}}` line in the stream: a byte cursor that lands
//! mid-line hands the next reader a headless fragment, and one unparseable line
//! is worth more noise downstream than the loss it stands for.
//!
//! # Exit
//!
//! Order is the contract, and it runs from the record nothing can miss to the
//! one only an attached client sees:
//!
//! 1. claude's stderr tail lands in `--stderr-file` — its last words, which are
//!    the only explanation of a crash that survives;
//! 2. the tombstone lands in `--exit-file`;
//! 3. *then* the stdout stream is closed, so a client that sees EOF can read the
//!    tombstone and find it already there;
//! 4. a short linger, so an attached client drains the tail;
//! 5. the socket is unlinked, and only while the path still names the socket
//!    this shim bound.
//!
//! Steps 2 and 3 in that order are what makes [`attach`]'s corpse guard
//! airtight: a tombstone that exists means claude is gone, so a socket that is
//! still answering during the linger cannot be mistaken for a live session.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::Parser;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, watch, Mutex as AsyncMutex};

use crate::process_env::Env;
use crate::procs;

// ------------------------------------------------------------------ protocol

/// The first line the shim sends, keyed by this.
const HELLO: &str = "shim_hello";

/// The line the shim splices in where output was lost.
const GAP: &str = "shim_gap";

/// The one line the shim reads rather than forwards.
///
/// Closing claude's stdin is the polite way to end a session — claude finishes
/// what it is doing and exits zero — and it is also exactly what the shim must
/// *never* do when a client disappears. Those two cannot both be "the socket
/// closed", so asking for it is explicit.
///
/// This costs the byte pump its purity: the client's side of the stream is read
/// a line at a time and one line is compared before being passed on. Lossless,
/// because that side of the stream is stream-json and therefore lines already —
/// the bytes forwarded are the bytes received, newline included — and safe from
/// collision, because the key is the shim's own and could never appear as a
/// stream-json request claude would accept.
pub const CLOSE_STDIN: &str = "{\"shim_stdin\":\"close\"}";

// ----------------------------------------------------------------- constants

/// How much of claude's stdout is kept for a client that is not there.
///
/// 100 MiB, which is a lot, and is meant to be: the window this exists for is a
/// server upgrade, and a turn that spends it printing must not come back
/// truncated. It is a cap on a `VecDeque` that starts empty, so an idle session
/// pays nothing for it.
pub const DEFAULT_BUFFER_BYTES: usize = 100 * 1024 * 1024;

/// How much of claude's stderr is kept for the tombstone.
const STDERR_TAIL: usize = 64 * 1024;

/// One read from claude's stdout.
const READ_CHUNK: usize = 64 * 1024;

/// The most the shim will hand a client in one write.
const SEND_CHUNK: usize = 256 * 1024;

/// How long the shim waits for clients to drain after claude exits.
const LINGER: Duration = Duration::from_secs(15);

/// How often the linger loop looks again.
const LINGER_POLL: Duration = Duration::from_millis(25);

/// How long an idle replay task waits before checking it is still the client.
const STALE_CHECK: Duration = Duration::from_secs(1);

/// How long a client that has connected has to send its cursor line.
///
/// A shim that never times this out leaks a task per stalled peer, and the peers
/// it must survive are servers being killed mid-handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long claude's pipes are given to reach EOF after it exits.
///
/// Bounded, because they may never: a `Bash` tool call that left something
/// running in the background holds the write end of claude's stdout open for as
/// long as *it* lives, and the shim cannot wait that out — a client waiting for
/// the stream to close would wait with it.
const DRAIN_GRACE: Duration = Duration::from_millis(500);

/// How long a client waits between dials of a socket that is not there yet.
const DIAL_RETRY: Duration = Duration::from_millis(50);

/// How long [`attach`] gives a socket to answer.
///
/// Short: this runs at boot with a whole instance directory to get through, and
/// a shim that has wedged must not hold it up. A shim that is alive answers in
/// microseconds.
pub const ATTACH_PATIENCE: Duration = Duration::from_millis(800);

/// How long [`spawn_via_shim`] waits for a shim it just started to bind.
///
/// Long, and for one reason: the shim binds *after* it has spawned claude, so
/// this covers a cold `claude` binary being paged in on a loaded machine.
pub const SPAWN_PATIENCE: Duration = Duration::from_secs(10);

// --------------------------------------------------------------- the sidecars

/// The three files one resident claude leaves in the instance directory.
///
/// ```text
/// <instance>/session-shims/<id>.stderr    claude's last words   (shim-owned)
/// <instance>/session-shims/<id>.exit      the tombstone         (shim-owned)
/// <instance>/session-shims/<short>.sock   the shim's socket     (shim-owned)
/// ```
///
/// All three belong to the shim, which is the difference from
/// [`crate::pty_shim::sidecar::Sidecars`]: a terminal's identity is the server's
/// to record because a browser has to be shown a list of them, whereas a
/// session's identity is already a session, recorded where sessions are. What is
/// here is only what the shim itself writes.
///
/// The socket's name is short and its directory is sometimes not this one — see
/// [`crate::pty_shim::sidecar`] for the `sun_path` budget that forces it.
#[derive(Debug, Clone)]
pub struct Sidecars {
    dir: PathBuf,
    id: String,
    socket: PathBuf,
}

impl Sidecars {
    /// The sidecars for a session id in a `session-shims` directory.
    pub fn new(dir: impl Into<PathBuf>, id: impl Into<String>) -> Self {
        let dir = dir.into();
        let id = id.into();
        let socket = crate::pty_shim::sidecar::socket_path(&dir, &id);
        Self { dir, id, socket }
    }

    /// The session id these sidecars belong to.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The unix socket the shim binds.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Where claude's last words go if it dies.
    pub fn stderr(&self) -> PathBuf {
        self.dir.join(format!("{}.stderr", self.id))
    }

    /// The tombstone the shim writes when claude exits.
    pub fn exit(&self) -> PathBuf {
        self.dir.join(format!("{}.exit", self.id))
    }

    /// Whether a tombstone exists at all, however malformed.
    ///
    /// The corpse guard asks this rather than [`Sidecars::read_tombstone`]: a
    /// half-written exit file still means claude is gone, and attaching to a
    /// corpse is worse than reporting an unknown status.
    pub fn tombstoned(&self) -> bool {
        self.exit().exists()
    }

    /// The tombstone, if claude has exited and left a readable one.
    pub fn read_tombstone(&self) -> Option<Exit> {
        serde_json::from_slice(&std::fs::read(self.exit()).ok()?).ok()
    }

    /// claude's last words, if it left any.
    pub fn read_stderr(&self) -> Option<String> {
        std::fs::read_to_string(self.stderr()).ok()
    }

    /// Make both directories, and clear the tombstone of whatever was here
    /// before.
    ///
    /// The tombstone has to go *before* the shim is spawned, not after it
    /// answers: a stale exit file from the last claude that used this id would
    /// make [`attach`]'s corpse guard — and the guard in [`spawn_via_shim`]
    /// itself — condemn a session that has only just started.
    pub fn prepare(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        crate::pty_shim::sidecar::ensure_socket_dir(&self.socket, &self.dir)?;
        match std::fs::remove_file(self.exit()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// What a session shim leaves behind when claude exits.
///
/// A status *or* a signal, never both and never neither, because that is what a
/// unix wait status is. Both are optional on the wire so a reader of a tombstone
/// written by an older shim is never surprised by a missing field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exit {
    /// claude's exit code, absent when a signal ended it.
    pub status: Option<i32>,
    /// The signal that ended claude, absent when it exited on its own.
    pub signal: Option<i32>,
    /// When it exited, ISO-8601.
    pub at: String,
}

// ------------------------------------------------------------ the shim process

/// What the shim was told to do.
#[derive(Debug, Clone)]
pub struct Args {
    /// Where to bind the unix socket.
    pub socket: PathBuf,
    /// Where to write claude's stderr tail.
    pub stderr_file: PathBuf,
    /// Where to write the tombstone.
    pub exit_file: PathBuf,
    /// How much stdout to keep for a client that is not attached.
    pub buffer_bytes: usize,
    /// The program to run.
    pub command: String,
    /// Its arguments, `argv[0]` excluded.
    pub args: Vec<String>,
}

/// Parse everything after the `session-shim` subcommand.
///
/// `--` is mandatory and is what separates the shim's own flags from the command
/// it is to run. The separator is not a nicety: a command line assembled without
/// one eventually gets a flag-shaped argument through to the wrong parser, and
/// claude's own command line is nothing but flags.
pub fn parse(argv: &[String]) -> Result<Args, String> {
    if let Some(token) = refused_spelling(argv) {
        return Err(format!("session-shim: unexpected argument {token:?}"));
    }
    let cli = SessionShimCli::try_parse_from(std::iter::once("session-shim".to_string()).chain(argv.iter().cloned()))
        .map_err(|error| error.to_string())?;
    let socket = cli.socket.ok_or_else(|| "session-shim: --socket is required".to_string())?;
    let stderr_file = cli.stderr_file.ok_or_else(|| "session-shim: --stderr-file is required".to_string())?;
    let exit_file = cli.exit_file.ok_or_else(|| "session-shim: --exit-file is required".to_string())?;
    // Required by hand, in the old order and with the old words: clap would
    // name the missing command first and say it differently.
    let mut rest = cli.command;
    if rest.is_empty() {
        return Err("session-shim: the command must follow `--`".to_string());
    }
    let command = rest.remove(0);
    Ok(Args {
        socket: PathBuf::from(socket),
        stderr_file: PathBuf::from(stderr_file),
        exit_file: PathBuf::from(exit_file),
        // A value the old parser could not read as a number is the same as one
        // that was not given.
        buffer_bytes: cli
            .buffer_bytes
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_BUFFER_BYTES),
        command,
        args: rest,
    })
}

/// The spelling this shim refused before clap: `--flag=value` was an
/// unexpected argument. The token after a value-taking flag is skipped: it was
/// that flag's value, whatever it looked like. The first other `--` is the
/// mandatory separator, where the command begins.
fn refused_spelling(argv: &[String]) -> Option<&str> {
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--" => return None,
            "--socket" | "--stderr-file" | "--exit-file" | "--buffer-bytes" => {
                iter.next();
            }
            other if is_attached(other) => return Some(other),
            _ => {}
        }
    }
    None
}

/// A flag and its value in one word.
fn is_attached(arg: &str) -> bool {
    ["--socket=", "--stderr-file=", "--exit-file=", "--buffer-bytes="]
        .iter()
        .any(|prefix| arg.starts_with(prefix))
}

/// `session-shim`'s command line: its flags, then the mandatory `--` and the
/// command it runs. `last` is what makes the separator mandatory and keeps
/// everything after it out of clap's flag parsing.
#[derive(Debug, Parser)]
#[command(
    name = "session-shim",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    args_override_self = true
)]
struct SessionShimCli {
    #[arg(long, allow_hyphen_values = true)]
    socket: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    stderr_file: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    exit_file: Option<String>,
    /// Undocumented, and here for the tests: overflowing a 100 MiB ring to
    /// prove the gap line appears would take a hundred megabytes.
    #[arg(long, allow_hyphen_values = true)]
    buffer_bytes: Option<String>,
    #[arg(last = true, num_args = 0..)]
    command: Vec<String>,
}

/// What one client's replay task is looking at.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    /// The client's cursor fell out of the ring; tell it so and move it on.
    Gap {
        /// The offset it was at.
        from: u64,
        /// The offset the ring now starts at.
        to: u64,
    },
    /// Bytes to send.
    Bytes(Vec<u8>),
    /// Nothing to send, and there may be more later.
    Idle,
    /// Nothing to send, and there never will be.
    Done,
}

/// claude's stdout, as much of it as is kept.
struct Ring {
    data: VecDeque<u8>,
    /// The absolute stream offset of `data[0]`.
    base: u64,
    /// How many bytes claude has ever written.
    total: u64,
    /// The cap `data` is trimmed back to.
    cap: usize,
    /// Whether the stream is closed for good.
    done: bool,
}

impl Ring {
    /// An empty ring holding at most `cap` bytes.
    fn new(cap: usize) -> Self {
        Self {
            data: VecDeque::new(),
            base: 0,
            total: 0,
            // A zero cap would make every push drop everything it just wrote and
            // the arithmetic below meaningless; one byte is the smallest ring
            // that still behaves like one.
            cap: cap.max(1),
            done: false,
        }
    }

    /// Record what claude wrote, dropping the oldest whole lines if it no longer
    /// fits.
    ///
    /// Whole lines, and that is the entire subtlety here. Trimming to an exact
    /// byte count leaves `base` in the middle of a line, so the next client to
    /// reattach is handed a fragment with no beginning — which is not a
    /// truncated message, it is an unparseable one, and it arrives *before* the
    /// gap line that would explain it. So the trim runs on to the next newline
    /// and `base` always points at the start of a line.
    ///
    /// A ring with no newline in it at all is a single line longer than the
    /// whole buffer, which is pathological and is discarded outright: its
    /// beginning left the window long ago, so keeping the tail cannot make it
    /// parseable again.
    fn push(&mut self, bytes: &[u8]) {
        self.data.extend(bytes);
        self.total += bytes.len() as u64;
        if self.data.len() <= self.cap {
            return;
        }
        let over = self.data.len() - self.cap;
        let cut = self
            .data
            .iter()
            .skip(over)
            .position(|byte| *byte == b'\n')
            .map_or(self.data.len(), |index| over + index + 1);
        self.data.drain(..cut);
        self.base += cut as u64;
    }

    /// What a client sitting at `cursor` should be sent next.
    fn step(&self, cursor: u64, limit: usize) -> Step {
        if cursor < self.base {
            return Step::Gap { from: cursor, to: self.base };
        }
        let available = self.total.saturating_sub(cursor) as usize;
        if available > 0 {
            return Step::Bytes(self.slice_from((cursor - self.base) as usize, available.min(limit)));
        }
        if self.done {
            Step::Done
        } else {
            Step::Idle
        }
    }

    /// `take` bytes from offset `start` within the ring.
    ///
    /// Through `as_slices` rather than the iterator, because a client that has
    /// fallen a hundred megabytes behind would otherwise pay for walking to its
    /// own position on every single read.
    fn slice_from(&self, start: usize, take: usize) -> Vec<u8> {
        let (head, tail) = self.data.as_slices();
        let mut out = Vec::with_capacity(take);
        if start < head.len() {
            let n = take.min(head.len() - start);
            out.extend_from_slice(&head[start..start + n]);
        }
        if out.len() < take {
            let from = start.saturating_sub(head.len());
            let n = (take - out.len()).min(tail.len().saturating_sub(from));
            out.extend_from_slice(&tail[from..from + n]);
        }
        out
    }
}

/// Everything one shim shares between its tasks.
struct Shim {
    ring: Mutex<Ring>,
    /// claude's stdout total, resent on every change so replay tasks can wait
    /// rather than poll.
    grew: watch::Sender<u64>,
    /// claude's stdin, and the one thing in here a client can take away.
    stdin: AsyncMutex<Option<ChildStdin>>,
    /// Bumped for every client accepted, so a stale one stops writing.
    ///
    /// Two clients forwarding at once would interleave bytes mid-line into a
    /// stdin that is parsed as one JSON object per line. The newer connection
    /// wins, because the older one is by definition a server that has gone.
    generation: AtomicU64,
    /// The highest offset any client has been sent, for the exit linger.
    drained_to: AtomicU64,
    /// Whether anybody has ever attached.
    ///
    /// Without it, a claude that dies before its server has finished connecting
    /// — a bad `--resume` id is the everyday case — would find no clients, drain
    /// instantly and unlink a socket the server is at that moment dialling.
    attached: AtomicBool,
    /// claude's pid, which is also its process group.
    claude_pid: u32,
}

impl Shim {
    /// Record that a client has been sent everything up to `cursor`.
    fn note_drained(&self, cursor: u64) {
        self.drained_to.fetch_max(cursor, Ordering::SeqCst);
    }

    /// Close the stream for good and wake everyone waiting on it.
    fn finish(&self) {
        let total = {
            let mut ring = self.ring.lock().expect("session-shim ring poisoned");
            ring.done = true;
            ring.total
        };
        self.grew.send_replace(total);
    }
}

/// Run one shim to completion: spawn claude, serve it, bury it.
pub async fn run(args: Args) -> Result<(), String> {
    // The environment and working directory are this process's, inherited:
    // whoever spawned the shim scrubbed them once, and a second copy of that
    // policy here could disagree with it after an upgrade.
    let mut command = Command::new(&args.command);
    command
        .args(&args.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // claude gets a process group of its own, so the interrupt ladder can
        // reach every process a tool call started and nothing of the shim's, and
        // so a signal aimed at the shim never takes claude down implicitly.
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("session-shim: cannot spawn {:?}: {error}", args.command))?;
    let claude_pid = child.id().unwrap_or(0);
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    // Bind before anything else can be told about this session: whoever spawned
    // us starts dialling the moment `spawn` returns.
    let _ = std::fs::remove_file(&args.socket);
    let listener = bind_socket(&args.socket, args.stderr_file.parent())?;
    // Recorded now, compared at unlink time: see `unlink_if_ours`.
    let identity = crate::pty_shim::child::path_identity(&args.socket);

    let (grew, _) = watch::channel(0u64);
    let shim = Arc::new(Shim {
        ring: Mutex::new(Ring::new(args.buffer_bytes)),
        grew,
        stdin: AsyncMutex::new(Some(stdin)),
        generation: AtomicU64::new(0),
        drained_to: AtomicU64::new(0),
        attached: AtomicBool::new(false),
        claude_pid,
    });

    let mut recording = tokio::spawn(record_stdout(stdout, shim.clone()));
    let tail = Arc::new(Mutex::new(VecDeque::new()));
    let keeping = tokio::spawn(keep_stderr_tail(stderr, tail.clone()));
    // Accepting runs for the shim's whole life, the linger included: a server
    // that connects *after* claude died still has to be told that it did.
    let accepting = tokio::spawn(accept_loop(listener, shim.clone()));

    let status = child
        .wait()
        .await
        .map_err(|error| format!("session-shim: cannot wait for {:?}: {error}", args.command))?;

    // Order from here to the end of the function is the contract. Nothing below
    // may be reordered without breaking a restart that lands in the middle of it.

    // 1. Both pipes get a bounded moment to reach EOF, so `total` is final and
    //    the stderr tail is whole before either is reported. Bounded because a
    //    grandchild that inherited them may hold either open indefinitely.
    let _ = tokio::time::timeout(DRAIN_GRACE, &mut recording).await;
    let _ = tokio::time::timeout(DRAIN_GRACE, keeping).await;

    // 2. claude's last words, which are the only account of a crash that
    //    survives the process that crashed.
    write_stderr_tail(&args.stderr_file, &tail);

    // 3. The tombstone, before the stream closes, so a client that reacts to EOF
    //    by reading it always finds it there.
    write_tombstone(&args.exit_file, &status);

    // 4. And only now the half-close, which is how an attached client learns
    //    claude is gone.
    recording.abort();
    shim.finish();

    // 5. Linger, so an attached client drains the tail — and so a server that
    //    has not managed to connect yet still can.
    let deadline = std::time::Instant::now() + LINGER;
    while std::time::Instant::now() < deadline {
        let total = shim.ring.lock().expect("session-shim ring poisoned").total;
        let waiting = !shim.attached.load(Ordering::SeqCst) || shim.drained_to.load(Ordering::SeqCst) < total;
        if !waiting {
            break;
        }
        tokio::time::sleep(LINGER_POLL).await;
    }

    // 6. The socket last, and guarded: by this point a successor shim may have
    //    bound a fresh one at the same path, and unlinking by name would strand
    //    a session that nothing is wrong with.
    accepting.abort();
    crate::pty_shim::child::unlink_if_ours(&args.socket, identity);
    Ok(())
}

/// Bind the session socket, making its directory again if it has just gone.
///
/// The spawner made the directory a moment ago, so the ordinary path is one
/// `bind`. The retry is for the same race [`crate::pty_shim::sidecar::
/// ensure_socket_dir`] documents from the other side: the `/tmp` directory is
/// shared by an instance's shims and its last user removes it on the way out, so
/// an outgoing shim can take it between the spawner making it and this `bind`.
/// Losing a whole session — that is what a shim failing to start costs — to a
/// tidy-up two processes away is not a trade worth making.
fn bind_socket(socket: &Path, beside: Option<&Path>) -> Result<UnixListener, String> {
    let failed = |error: std::io::Error| format!("session-shim: cannot bind {}: {error}", socket.display());
    match UnixListener::bind(socket) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            crate::pty_shim::sidecar::ensure_socket_dir(socket, beside.unwrap_or(Path::new("")))
                .map_err(|error| format!("session-shim: cannot make the directory for {}: {error}", socket.display()))?;
            UnixListener::bind(socket).map_err(failed)
        }
        settled => settled.map_err(failed),
    }
}

/// Read claude's stdout into the ring for as long as it produces any.
///
/// Deliberately does not close the stream when the pipe ends: only [`run`]
/// does that, and only after the tombstone is on disk.
async fn record_stdout(mut stdout: ChildStdout, shim: Arc<Shim>) {
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        let read = match stdout.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let total = {
            let mut ring = shim.ring.lock().expect("session-shim ring poisoned");
            ring.push(&buf[..read]);
            ring.total
        };
        shim.grew.send_replace(total);
    }
}

/// Keep the last [`STDERR_TAIL`] bytes of claude's stderr.
async fn keep_stderr_tail(mut stderr: ChildStderr, tail: Arc<Mutex<VecDeque<u8>>>) {
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        let read = match stderr.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let mut tail = tail.lock().expect("session-shim stderr tail poisoned");
        tail.extend(&buf[..read]);
        let len = tail.len();
        if len > STDERR_TAIL {
            tail.drain(..len - STDERR_TAIL);
        }
    }
}

/// Write what was kept of claude's stderr.
fn write_stderr_tail(path: &Path, tail: &Mutex<VecDeque<u8>>) {
    let bytes: Vec<u8> = tail.lock().expect("session-shim stderr tail poisoned").iter().copied().collect();
    if let Err(error) = std::fs::write(path, bytes) {
        tracing::error!(path = %path.display(), %error, "session-shim: cannot write the stderr file");
    }
}

/// Write the tombstone, the one record that outlives everything else.
fn write_tombstone(path: &Path, status: &ExitStatus) {
    use std::os::unix::process::ExitStatusExt;
    let exit = Exit {
        status: status.code(),
        signal: status.signal(),
        at: crate::sessions::iso8601(std::time::SystemTime::now()),
    };
    match serde_json::to_vec(&exit) {
        Ok(encoded) => {
            if let Err(error) = std::fs::write(path, encoded) {
                tracing::error!(path = %path.display(), %error, "session-shim: cannot write the exit file");
            }
        }
        Err(error) => tracing::error!(%error, "session-shim: cannot encode the exit file"),
    }
}

/// Accept clients for as long as the shim lives.
async fn accept_loop(listener: UnixListener, shim: Arc<Shim>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(serve_client(stream, shim.clone()));
            }
            Err(error) => {
                tracing::warn!(%error, "session-shim: accept failed");
                // A listener that cannot accept will not start again on its own;
                // claude is still fine, so keep it and let the next boot decide.
                tokio::time::sleep(LINGER_POLL).await;
            }
        }
    }
}

/// Serve one client: handshake, then a stream in each direction.
async fn serve_client(stream: UnixStream, shim: Arc<Shim>) {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);

    let mut first = String::new();
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, reader.read_line(&mut first)).await {
        Ok(Ok(read)) if read > 0 => {}
        _ => return,
    }
    let (cursor, total) = {
        let ring = shim.ring.lock().expect("session-shim ring poisoned");
        let asked = serde_json::from_str::<serde_json::Value>(&first)
            .ok()
            .and_then(|value| value.get("cursor").and_then(serde_json::Value::as_u64))
            .unwrap_or(0);
        // Clamped, because a cursor past the end of the stream is a client whose
        // record disagrees with reality, and the only defensible answer is
        // "everything from here on".
        (asked.min(ring.total), ring.total)
    };
    let hello = format!("{{\"{HELLO}\":{{\"claude_pid\":{},\"total\":{total}}}}}\n", shim.claude_pid);
    if write.write_all(hello.as_bytes()).await.is_err() {
        return;
    }
    let _ = write.flush().await;

    // Only now does this connection count as the client — after it has proved it
    // speaks the protocol. Taking the generation at accept time instead would let
    // anything that opens the socket and says nothing lock the live server out of
    // claude's stdin for as long as it holds the connection.
    let generation = shim.generation.fetch_add(1, Ordering::SeqCst) + 1;
    shim.attached.store(true, Ordering::SeqCst);

    let replaying = tokio::spawn(replay(shim.clone(), write, cursor, generation));
    // The same `BufReader` the hello line came out of: a client that pipelines
    // its first prompt behind the handshake has already been read into that
    // buffer, and rebuilding the reader here would drop it on the floor.
    forward(reader, &shim, generation).await;
    // `forward` returns when the client is gone, and a replay task with nothing
    // to say would otherwise wait for output that has nobody left to go to.
    replaying.abort();
}

/// Send claude's stdout to one client, from `cursor` on, for as long as it will
/// take it.
async fn replay(shim: Arc<Shim>, mut write: OwnedWriteHalf, mut cursor: u64, generation: u64) {
    let mut grew = shim.grew.subscribe();
    loop {
        if shim.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        let step = shim.ring.lock().expect("session-shim ring poisoned").step(cursor, SEND_CHUNK);
        match step {
            Step::Gap { from, to } => {
                let note = format!("{{\"{GAP}\":{{\"from\":{from},\"to\":{to}}}}}\n");
                if write.write_all(note.as_bytes()).await.is_err() {
                    return;
                }
                cursor = to;
            }
            Step::Bytes(bytes) => {
                cursor += bytes.len() as u64;
                if write.write_all(&bytes).await.is_err() {
                    return;
                }
            }
            Step::Idle => {
                // Woken by the ring growing or closing, so the timeout is not how
                // output gets delivered — it is only how a task whose client has
                // been replaced finds out, on a session too quiet to wake it any
                // other way. Seconds rather than milliseconds because that is all
                // the urgency reaping a stale task deserves. (`changed` cannot end
                // any other way: the sender lives in the `Shim` this task holds an
                // `Arc` to.)
                let _ = tokio::time::timeout(STALE_CHECK, grew.changed()).await;
                continue;
            }
            Step::Done => {
                shim.note_drained(cursor);
                // Half-close rather than drop: the client learns claude is gone
                // now, not when the linger runs out.
                let _ = write.shutdown().await;
                return;
            }
        }
        let _ = write.flush().await;
        shim.note_drained(cursor);
    }
}

/// Send one client's input to claude's stdin, verbatim.
///
/// EOF here means the server went away, and claude's stdin must **never** be
/// closed because of it — that would end the session, which is the one thing
/// this whole module exists to prevent.
async fn forward(mut reader: BufReader<OwnedReadHalf>, shim: &Shim, generation: u64) {
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if shim.generation.load(Ordering::SeqCst) != generation {
            // Replaced by a newer client, which owns stdin now.
            return;
        }
        if String::from_utf8_lossy(&line).trim() == CLOSE_STDIN {
            // Taken, not just closed: dropping the handle is what claude sees as
            // EOF, and leaving `None` behind is what makes a second request, or
            // a later write from a reconnected client, a no-op rather than a
            // panic.
            shim.stdin.lock().await.take();
            continue;
        }
        let mut held = shim.stdin.lock().await;
        let Some(stdin) = held.as_mut() else { continue };
        if stdin.write_all(&line).await.is_err() {
            return;
        }
        let _ = stdin.flush().await;
    }
}

// ------------------------------------------------------------- the server end

/// Why a link to a session shim could not be established.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    /// The shim could not be started at all.
    #[error("cannot start a session shim: {0}")]
    Spawn(std::io::Error),
    /// Its sidecar directory could not be prepared.
    #[error("cannot prepare the session shim files: {0}")]
    Sidecars(std::io::Error),
    /// The socket is not there, or refused the connection.
    #[error("cannot reach the session shim: {0}")]
    Connect(std::io::Error),
    /// The socket accepted but the handshake did not complete.
    #[error("the session shim did not complete its handshake")]
    Handshake,
    /// The shim is still answering, but the claude behind it is dead.
    ///
    /// A shim lingers after claude exits so an attached client can drain the
    /// tail, which means a socket alone proves nothing. Attaching to one of
    /// these would replay a stale exit and report a session as having just died
    /// when it in fact died before the restart.
    #[error("the session shim is minding a dead claude: {0}")]
    Corpse(String),
}

/// What claude said, one line at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// One line of claude's stdout, newline stripped.
    Line(String),
    /// Output was lost while nothing was attached.
    ///
    /// The bytes between these two offsets overflowed the shim's ring. Whole
    /// lines, always — see [`Ring::push`] — so what follows is parseable even
    /// though it is not complete.
    Gap {
        /// The first offset that was lost.
        from: u64,
        /// The first offset that was not.
        to: u64,
    },
}

/// A live link to one session shim.
pub struct Link {
    claude_pid: u32,
    shim_pid: Option<u32>,
    socket: PathBuf,
    total: u64,
    /// How far through claude's stdout the last [`Link::recv`] left this client.
    ///
    /// Carried on the channel rather than kept by the reader task, because the
    /// two run at different speeds: the reader is normally several lines ahead
    /// of whoever is draining it, and a counter it bumped as it read would say
    /// "caught up" while the backlog was still in the channel. See
    /// [`Link::replaying`], which is the reason any of this is counted.
    at: u64,
    inbound: mpsc::UnboundedReceiver<(Output, u64)>,
    writer: OwnedWriteHalf,
    /// The task that owns the read half. Aborted with this struct, so dropping a
    /// link really does close the socket — a reader parked on a shim that has
    /// nothing to say would otherwise hold it open, and the shim would wait out
    /// its whole linger for a client that is already gone.
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link")
            .field("claude_pid", &self.claude_pid)
            .field("shim_pid", &self.shim_pid)
            .field("socket", &self.socket)
            .field("total", &self.total)
            .field("at", &self.at)
            .finish()
    }
}

impl Link {
    /// The pid of the claude on the other end, which is also its process group.
    pub fn claude_pid(&self) -> u32 {
        self.claude_pid
    }

    /// The shim's own pid, when this link came from [`spawn_via_shim`].
    ///
    /// `None` after an [`attach`]: the shim that answered was started by a
    /// server that is gone, and the wire says nothing about it. Nothing needs
    /// it — killing a session goes through [`Link::kill_claude`], which is aimed
    /// at claude's process group and not at the shim.
    pub fn shim_pid(&self) -> Option<u32> {
        self.shim_pid
    }

    /// The socket this link is over.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// How much claude had written when this link was made.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// How far through claude's stdout the last [`Link::recv`] left this client.
    ///
    /// The offset **after** the item just delivered, so a caller telling replay
    /// from live compares it against [`Link::total`]: everything up to and
    /// including the item that reaches `total` is history, and the next item is
    /// the first thing claude has said since this link was made.
    ///
    /// An [`attach`] asks for cursor zero, so the first thing a reattached
    /// client sees is claude's whole stdout from the beginning of the session —
    /// every `result`, every notification, replayed. Acting on those a second
    /// time would mean a phone buzzing once per turn the session ever had, which
    /// is why the boundary has to be exact rather than nearly right. A fresh
    /// [`spawn_via_shim`] has `total` zero and is therefore never replaying.
    pub fn at(&self) -> u64 {
        self.at
    }

    /// The next thing claude said, or `None` once the link is gone.
    ///
    /// `None` after an exit means the shim half-closed the stream, and by the
    /// contract in the module docs the tombstone is already on disk.
    ///
    /// Cancellation-safe: the line framing happens in the reader task, so what
    /// is polled here is a channel.
    pub async fn recv(&mut self) -> Option<Output> {
        let (output, at) = self.inbound.recv().await?;
        self.at = at;
        Some(output)
    }

    /// Send one line to claude's stdin. An error means the link must be rebuilt.
    ///
    /// The newline is added here if the caller did not bring one: a stream-json
    /// request that arrives without its terminator leaves claude waiting for the
    /// rest of a line that will never come.
    pub async fn send_line(&mut self, line: &str) -> std::io::Result<()> {
        self.writer.write_all(line.trim_end_matches('\n').as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await
    }

    /// Ask the shim to close claude's stdin.
    ///
    /// The polite end of a session: claude finishes what it is doing, writes its
    /// last result and exits zero. There is no way back — a shim that has
    /// dropped the handle cannot get another one — so this is the last thing a
    /// caller says over a link.
    pub async fn close_stdin(&mut self) -> std::io::Result<()> {
        self.writer.write_all(CLOSE_STDIN.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await
    }

    /// Signal claude's whole process group.
    ///
    /// The group, not the pid, because a turn's real work is in the processes a
    /// tool call started; and through [`procs::signal_group`], which refuses the
    /// two pids that are not pids — `kill(-0, …)` is this server's own group and
    /// `kill(-1, …)` is every process it may signal.
    pub fn kill_claude(&self, signal: libc::c_int) {
        procs::signal_group(self.claude_pid, signal);
    }
}

/// What [`spawn_via_shim`] needs to know.
#[derive(Debug)]
pub struct Spawn<'a> {
    /// Where the shim's three files go.
    pub sidecars: &'a Sidecars,
    /// The claude binary to run.
    pub program: &'a str,
    /// Its arguments — the whole stream-json command line.
    pub args: &'a [String],
    /// The directory claude runs in.
    ///
    /// Applied as given. A caller that wants a fallback for a directory the user
    /// has since deleted has to name one — see [`super::claude_recipe`] — because
    /// the only fallback available *here* is the server's own working directory,
    /// and starting somebody's agent in the source tree of the server driving it
    /// is worse than not starting it at all.
    pub workdir: Option<&'a Path>,
    /// The environment claude runs with. Applied with `env_clear` first, so this
    /// is the whole of it.
    pub env: &'a Env,
    /// How much stdout to keep for a client that is not attached; the default is
    /// [`DEFAULT_BUFFER_BYTES`].
    pub buffer_bytes: Option<usize>,
}

/// Start a shim, and through it a claude, and connect to it.
///
/// Every flag on the spawn is load-bearing. `process_group(0)` takes the shim
/// out of the server's process group, so a `kill` aimed at the server — or a
/// shell that signals the whole group on Ctrl-C — does not take the sessions
/// with it. `kill_on_drop(false)` keeps tokio from tidying it away when the
/// handle goes. Null stdio means the shim never holds a descriptor onto anything
/// the server owns; it speaks over its socket and nothing else.
pub async fn spawn_via_shim(spawn: Spawn<'_>) -> Result<Link, LinkError> {
    spawn.sidecars.prepare().map_err(LinkError::Sidecars)?;

    let mut argv: Vec<OsString> = vec![
        "--socket".into(),
        spawn.sidecars.socket().into(),
        "--stderr-file".into(),
        spawn.sidecars.stderr().into(),
        "--exit-file".into(),
        spawn.sidecars.exit().into(),
    ];
    if let Some(bytes) = spawn.buffer_bytes {
        argv.extend(["--buffer-bytes".into(), bytes.to_string().into()]);
    }
    argv.push("--".into());
    argv.push(spawn.program.into());
    argv.extend(spawn.args.iter().map(OsString::from));
    let mut command = shim_command(&argv, spawn.env);
    // Set unconditionally. Skipping a directory that has gone missing would not
    // make the spawn fail — it would make it *succeed somewhere else*, namely
    // wherever the server happens to be running, which for a developer is the
    // checkout they are working in. A launch failure is the honest answer, and
    // the caller has already substituted a directory it is willing to name.
    if let Some(dir) = spawn.workdir {
        command.current_dir(dir);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(false)
        .process_group(0);

    let mut child = command.spawn().map_err(LinkError::Spawn)?;
    let shim_pid = child.id();
    // Reaped explicitly rather than left to the orphan queue: while this server
    // lives the shim is its child, and an unreaped exit is a zombie for as long
    // as the server runs.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });

    let mut link = connect(spawn.sidecars.socket(), SPAWN_PATIENCE).await?;
    link.shim_pid = shim_pid;
    Ok(link)
}

/// The command that starts a shim told `argv`, in exactly `env`: this server
/// under its `session-shim` subcommand, with `env_clear` first.
#[cfg(not(test))]
fn shim_command(argv: &[OsString], env: &Env) -> Command {
    let mut command = Command::new(crate::pty_shim::shim_program());
    command.arg("session-shim").args(argv).env_clear().envs(env);
    command
}

/// The same, in this crate's unit tests: the test binary itself, running the
/// one test that is a shim.
///
/// [`crate::pty_shim::shim_program`] cannot serve a unit test. From a test
/// binary it falls back to the server binary beside it in `target/debug` —
/// which `cargo test --lib` never builds, and which any other run may have left
/// there from older source. Absent, the test binary itself was run as
/// `session-shim --socket …`, libtest refused the option and exited 101, and
/// every respawn in [`super::resident`]'s tests timed out on a socket nobody
/// would bind. Present, the shim under test was whatever that binary happened
/// to be. The test binary is always there, and always this source.
#[cfg(test)]
fn shim_command(argv: &[OsString], env: &Env) -> Command {
    tests::this_binary_as_the_shim(argv, env)
}

/// Reconnect to the shim of a session that survived a restart.
///
/// The corpse guard is the whole of the difference from [`connect`], and it asks
/// twice for a reason. The tombstone is checked before dialling because a socket
/// that is only lingering is not worth the round trip, and again after the
/// handshake because claude may have exited during it — and only then can
/// claude's pid be probed at all, since the hello is where it comes from.
pub async fn attach(sidecars: &Sidecars) -> Result<Link, LinkError> {
    let corpse = |why: &str| LinkError::Corpse(format!("{}: {why}", sidecars.id()));
    if sidecars.tombstoned() {
        return Err(corpse("it has already written a tombstone"));
    }
    let link = connect(sidecars.socket(), ATTACH_PATIENCE).await?;
    if sidecars.tombstoned() {
        return Err(corpse("it wrote a tombstone while we were connecting"));
    }
    if !procs::alive(link.claude_pid) {
        return Err(corpse(&format!("claude {} is not running", link.claude_pid)));
    }
    Ok(link)
}

/// Dial a shim this process is hosting itself.
///
/// The real path re-execs this binary under its `session-shim` subcommand, and a
/// unit-test harness binary has no such subcommand — so [`super::resident`]'s
/// tests run [`run`] as a task for the process a harness starts with, and dial
/// it here. Everything above the fork is still the real thing: the wire, the
/// ring, the tombstone, and a real child process on the other end of the pipes.
/// (A respawn in those tests does get a shim process of its own, through
/// [`shim_command`].)
#[cfg(test)]
pub(super) async fn connect_in_process(socket: &Path) -> Result<Link, LinkError> {
    connect(socket, SPAWN_PATIENCE).await
}

/// Dial a shim and do the handshake.
///
/// The cursor is always zero — see the module docs on why replaying everything
/// is the correctness anchor rather than a cost.
async fn connect(socket: &Path, patience: Duration) -> Result<Link, LinkError> {
    let deadline = tokio::time::Instant::now() + patience;
    let stream = loop {
        match UnixStream::connect(socket).await {
            Ok(stream) => break stream,
            Err(error) if tokio::time::Instant::now() >= deadline => return Err(LinkError::Connect(error)),
            Err(_) => tokio::time::sleep(DIAL_RETRY).await,
        }
    };
    let (read, mut writer) = stream.into_split();
    let mut reader = BufReader::new(read);

    let handshake = async {
        writer.write_all(b"{\"cursor\":0}\n").await.ok()?;
        writer.flush().await.ok()?;
        let mut hello = String::new();
        reader.read_line(&mut hello).await.ok()?;
        let value = serde_json::from_str::<serde_json::Value>(&hello).ok()?;
        let hello = value.get(HELLO)?;
        let pid = hello.get("claude_pid").and_then(serde_json::Value::as_u64)?;
        let total = hello.get("total").and_then(serde_json::Value::as_u64).unwrap_or(0);
        Some((pid as u32, total))
    };
    let (claude_pid, total) = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake)
        .await
        .map_err(|_| LinkError::Handshake)?
        .ok_or(LinkError::Handshake)?;

    // A dedicated reader from here on, over the very `BufReader` the hello came
    // out of: the shim starts replaying the moment it has answered, so the
    // backlog is very often already sitting in that buffer. Rebuilding the
    // reader would drop it, and what it drops is the head of a line.
    let (inbound_tx, inbound) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        // Counted off the bytes actually read rather than off the string handed
        // on, so a line that ended `\r\n` — or anything else the trimming below
        // hides — still advances the position by what it really occupied. This
        // has to agree with the shim's own offsets exactly: it is what tells a
        // reattached client where the replay stops.
        let mut at = 0u64;
        let mut line = String::new();
        loop {
            line.clear();
            let read = match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => return,
                Ok(read) => read as u64,
            };
            let text = line.trim_end_matches('\n').trim_end_matches('\r');
            let output = classify(text);
            // A gap line is the shim's own, not claude's: it occupies no space
            // in the stream it describes, and the offset it carries is where
            // claude's own bytes resume.
            at = match &output {
                Output::Gap { to, .. } => *to,
                Output::Line(_) => at + read,
            };
            if inbound_tx.send((output, at)).is_err() {
                return;
            }
        }
    });

    Ok(Link {
        claude_pid,
        shim_pid: None,
        socket: socket.to_path_buf(),
        total,
        at: 0,
        inbound,
        writer,
        reader: task,
    })
}

/// Tell the shim's own gap line apart from a line claude wrote.
///
/// A cheap prefix test before the parse, because every other line on this stream
/// is claude's and the caller is about to parse it properly anyway.
fn classify(text: &str) -> Output {
    if text.starts_with("{\"shim_gap\"") {
        if let Some(gap) = serde_json::from_str::<serde_json::Value>(text).ok().and_then(|value| {
            let gap = value.get(GAP)?;
            let from = gap.get("from").and_then(serde_json::Value::as_u64)?;
            let to = gap.get("to").and_then(serde_json::Value::as_u64)?;
            Some(Output::Gap { from, to })
        }) {
            return gap;
        }
    }
    Output::Line(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where [`this_binary_as_the_shim`] puts the shim's own arguments, as a
    /// JSON array: a libtest command line has no room for them.
    const SHIM_ARGV: &str = "DROPPEN_UNIT_TEST_SHIM_ARGV";

    /// This test binary as the session shim: libtest asked for [`session_shim`]
    /// and nothing else, the shim's arguments in [`SHIM_ARGV`]. See
    /// [`super::shim_command`] for why a unit test cannot run the server's.
    pub(super) fn this_binary_as_the_shim(argv: &[OsString], env: &Env) -> Command {
        let argv: Vec<String> = argv
            .iter()
            .map(|arg| arg.to_str().expect("a unit test's shim arguments are UTF-8").to_owned())
            .collect();
        // libtest names a test by its path inside the crate.
        let (_crate, module) = module_path!().split_once("::").expect("a module inside a crate");
        let mut command = Command::new(std::env::current_exe().expect("this test binary"));
        command
            .args(["--ignored", "--exact", "--quiet", &format!("{module}::session_shim")])
            .env_clear()
            .envs(env)
            .env(SHIM_ARGV, serde_json::to_string(&argv).expect("json"));
        command
    }

    /// Not a test: the session shim, when [`this_binary_as_the_shim`] runs this
    /// binary as one. Run any other way it has no arguments, and returns.
    ///
    /// It exits rather than returns once the shim is done, with the code the
    /// server's `session-shim` would: the process is the shim, and libtest's
    /// summary after it would be a second ending.
    #[test]
    #[ignore = "the session shim a unit test's respawn runs, not a test"]
    fn session_shim() {
        let Ok(argv) = std::env::var(SHIM_ARGV) else {
            return;
        };
        let argv: Vec<String> = serde_json::from_str(&argv).expect("the shim's arguments");
        let outcome = parse(&argv).and_then(|args| {
            let runtime = tokio::runtime::Runtime::new().map_err(|error| format!("cannot start a runtime: {error}"))?;
            runtime.block_on(run(args))
        });
        std::process::exit(i32::from(outcome.is_err()));
    }

    /// A shim running in this process over a fake claude, and its files.
    struct Harness {
        _dir: tempfile::TempDir,
        sidecars: Sidecars,
        shim: tokio::task::JoinHandle<Result<(), String>>,
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            self.shim.abort();
        }
    }

    impl Harness {
        /// Start a shim over `/bin/sh -c script`, with a ring of `buffer_bytes`.
        async fn start(script: &str, buffer_bytes: usize) -> Harness {
            let dir = tempfile::tempdir().expect("tempdir");
            let sidecars = Sidecars::new(dir.path(), "0123456789abcdef");
            sidecars.prepare().expect("prepare");
            let args = Args {
                socket: sidecars.socket().to_path_buf(),
                stderr_file: sidecars.stderr(),
                exit_file: sidecars.exit(),
                buffer_bytes,
                command: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), script.to_string()],
            };
            let shim = tokio::spawn(run(args));
            Harness { _dir: dir, sidecars, shim }
        }

        /// Connect, retrying while the shim gets its socket up.
        async fn link(&self) -> Link {
            connect(self.sidecars.socket(), Duration::from_secs(5)).await.expect("the shim answers")
        }

        /// Wait for the tombstone, which is written before the stream closes.
        async fn wait_for_tombstone(&self) -> Exit {
            for _ in 0..200 {
                if let Some(exit) = self.sidecars.read_tombstone() {
                    return exit;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            panic!("no tombstone after 5 seconds");
        }
    }

    /// The next line, or a panic rather than a test that hangs forever.
    async fn next(link: &mut Link) -> Output {
        tokio::time::timeout(Duration::from_secs(5), link.recv())
            .await
            .expect("a line within five seconds")
            .expect("the link is still open")
    }

    // ------------------------------------------------------------ the parser

    #[test]
    fn the_command_line_needs_three_paths_and_a_separator() {
        let args = parse(&[
            "--socket".into(),
            "/tmp/a.sock".into(),
            "--stderr-file".into(),
            "/tmp/a.stderr".into(),
            "--exit-file".into(),
            "/tmp/a.exit".into(),
            "--".into(),
            "claude".into(),
            "--verbose".into(),
        ])
        .expect("a complete command line");
        assert_eq!(args.socket, PathBuf::from("/tmp/a.sock"));
        assert_eq!(args.stderr_file, PathBuf::from("/tmp/a.stderr"));
        assert_eq!(args.exit_file, PathBuf::from("/tmp/a.exit"));
        assert_eq!(args.command, "claude");
        assert_eq!(args.args, vec!["--verbose".to_string()]);
        assert_eq!(args.buffer_bytes, DEFAULT_BUFFER_BYTES, "the ring has a default");

        let missing = parse(&["--socket".into(), "/tmp/a.sock".into(), "--".into(), "claude".into()]);
        assert!(missing.is_err(), "every path is required: {missing:?}");

        let no_command = parse(&[
            "--socket".into(),
            "/tmp/a.sock".into(),
            "--stderr-file".into(),
            "/tmp/a.stderr".into(),
            "--exit-file".into(),
            "/tmp/a.exit".into(),
        ]);
        assert!(no_command.is_err(), "a shim with nothing to run is not a shim");
    }

    // -------------------------------------------------------------- the ring

    #[test]
    fn an_overflowing_ring_drops_whole_lines_and_never_half_of_one() {
        let mut ring = Ring::new(16);
        ring.push(b"aaaa\nbbbb\ncccc\n");
        assert_eq!(ring.base, 0, "it still fits");

        // 20 bytes in a 16-byte ring: four over, and the trim runs on from there
        // to the end of the line it lands in rather than stopping at four.
        ring.push(b"dddd\n");
        assert_eq!(ring.base, 5, "the whole of the first line went");
        assert_eq!(ring.data.iter().copied().collect::<Vec<u8>>(), b"bbbb\ncccc\ndddd\n".to_vec());
        assert_eq!(ring.total, 20);

        // Every byte still in the ring is reachable, and the first one is the
        // start of a line.
        let Step::Bytes(bytes) = ring.step(ring.base, 1024) else {
            panic!("there are bytes to send")
        };
        assert_eq!(bytes, b"bbbb\ncccc\ndddd\n".to_vec());
    }

    #[test]
    fn a_single_line_larger_than_the_whole_ring_is_discarded_outright() {
        let mut ring = Ring::new(8);
        ring.push(b"0123456789abcdef");
        assert!(ring.data.is_empty(), "a headless fragment is worth nothing to anybody");
        assert_eq!(ring.base, 16);
        assert_eq!(ring.total, 16);
    }

    #[test]
    fn a_cursor_that_fell_out_of_the_ring_is_told_so_before_it_is_told_anything_else() {
        let mut ring = Ring::new(8);
        ring.push(b"aaaa\nbbbb\n");
        assert_eq!(ring.step(0, 1024), Step::Gap { from: 0, to: 5 });
        assert_eq!(ring.step(5, 1024), Step::Bytes(b"bbbb\n".to_vec()));
        assert_eq!(ring.step(10, 1024), Step::Idle, "caught up, and claude is still there");
        ring.done = true;
        assert_eq!(ring.step(10, 1024), Step::Done);
    }

    #[test]
    fn the_ring_reads_across_the_seam_of_its_two_halves() {
        // Force a wrap: fill, drain, refill, so `as_slices` really is two.
        let mut ring = Ring::new(12);
        ring.push(b"aaaa\nbbbb\n");
        ring.push(b"cc\n");
        assert!(ring.base > 0, "something was trimmed");
        let Step::Bytes(bytes) = ring.step(ring.base, 1024) else {
            panic!("there are bytes to send")
        };
        assert_eq!(bytes.len(), (ring.total - ring.base) as usize);
        assert!(bytes.ends_with(b"cc\n"));
        // And a partial read of the same span agrees with it.
        let Step::Bytes(head) = ring.step(ring.base, 3) else {
            panic!("there are bytes to send")
        };
        assert_eq!(head, bytes[..3].to_vec());
    }

    // --------------------------------------------------------- the whole shim

    #[tokio::test]
    async fn a_client_says_hello_and_then_talks_to_claude_both_ways() {
        // `cat` is claude enough: everything in comes back out.
        let harness = Harness::start("exec /bin/cat", DEFAULT_BUFFER_BYTES).await;
        let mut link = harness.link().await;
        assert!(link.claude_pid() > 1, "the hello carries a real pid");
        assert_eq!(link.total(), 0, "nothing has been said yet");

        link.send_line("{\"type\":\"user\",\"n\":1}").await.expect("send");
        assert_eq!(next(&mut link).await, Output::Line("{\"type\":\"user\",\"n\":1}".into()));
        link.send_line("{\"type\":\"user\",\"n\":2}").await.expect("send");
        assert_eq!(next(&mut link).await, Output::Line("{\"type\":\"user\",\"n\":2}".into()));
    }

    #[tokio::test]
    async fn a_disconnect_leaves_claude_alone_and_a_reattach_replays_everything() {
        let harness = Harness::start("exec /bin/cat", DEFAULT_BUFFER_BYTES).await;
        let mut link = harness.link().await;
        let claude = link.claude_pid();
        link.send_line("one").await.expect("send");
        assert_eq!(next(&mut link).await, Output::Line("one".into()));

        // The server dies mid-session. This is the whole feature.
        drop(link);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(procs::alive(claude), "closing the socket must not close claude's stdin");

        let mut link = harness.link().await;
        assert_eq!(link.claude_pid(), claude, "the same claude, not a new one");
        assert_eq!(link.total(), 4, "the shim kept what claude said while nobody listened");
        assert_eq!(next(&mut link).await, Output::Line("one".into()), "replayed from zero");

        // And it is still a working session, not just a readable one.
        link.send_line("two").await.expect("send");
        assert_eq!(next(&mut link).await, Output::Line("two".into()));
    }

    #[tokio::test]
    async fn a_replaced_client_stops_writing_to_a_stdin_that_is_no_longer_its() {
        let harness = Harness::start("exec /bin/cat", DEFAULT_BUFFER_BYTES).await;
        let mut old = harness.link().await;
        old.send_line("from the old server").await.expect("send");
        assert_eq!(next(&mut old).await, Output::Line("from the old server".into()));

        // A new server attaches without the old one having noticed it is gone —
        // a partitioned or wedged predecessor, which is the everyday shape of an
        // upgrade that did not shut down cleanly.
        let mut new = harness.link().await;
        assert_eq!(next(&mut new).await, Output::Line("from the old server".into()), "replayed from zero");

        // The handshake above has already taken the generation, so what the old
        // one says next is deterministically too late.
        old.send_line("a ghost").await.expect("its socket is still open");
        new.send_line("from the new server").await.expect("send");
        assert_eq!(
            next(&mut new).await,
            Output::Line("from the new server".into()),
            "two writers into one stream-json stdin would interleave mid-line"
        );
    }

    #[tokio::test]
    async fn output_lost_while_nobody_was_attached_arrives_as_a_gap_line() {
        // Forty lines into a ring that holds thirty-two bytes: everything but
        // the tail of it is gone before anybody connects. `line-1\n` through
        // `line-9\n` are seven bytes each and the rest are eight.
        const WRITTEN: u64 = 9 * 7 + 31 * 8;
        let harness = Harness::start("for i in $(seq 1 40); do echo \"line-$i\"; done; exec /bin/cat", 32).await;

        // Wait for all of it to have been recorded rather than for a moment to
        // pass: how long a shell takes to run a loop is not this test's subject,
        // and a machine under load takes as long as it takes.
        let mut link = None;
        for _ in 0..200 {
            let candidate = harness.link().await;
            if candidate.total() >= WRITTEN {
                link = Some(candidate);
                break;
            }
            drop(candidate);
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let mut link = link.expect("claude wrote its forty lines within five seconds");
        assert_eq!(link.total(), WRITTEN);

        let Output::Gap { from, to } = next(&mut link).await else {
            panic!("the first thing a client that missed output is told is that it did")
        };
        assert_eq!(from, 0);
        assert!(to > 0 && to < WRITTEN, "a gap that ends inside the stream: {from}..{to}");

        // What survived is whole lines, up to and including the last one claude
        // wrote — the point of trimming to a newline rather than to a byte
        // count. And they account for exactly the bytes the gap did not.
        let mut seen: Vec<String> = Vec::new();
        loop {
            match next(&mut link).await {
                Output::Line(line) => {
                    let last = line == "line-40";
                    seen.push(line);
                    if last {
                        break;
                    }
                }
                Output::Gap { .. } => panic!("one gap, not a stutter of them"),
            }
        }
        assert!(
            seen.iter().all(|line| line.starts_with("line-")),
            "no headless fragment survived the trim: {seen:?}"
        );
        let kept: u64 = seen.iter().map(|line| line.len() as u64 + 1).sum();
        assert_eq!(to + kept, WRITTEN, "the gap ends exactly where the surviving lines begin");
    }

    #[tokio::test]
    async fn claude_exiting_writes_the_tombstone_before_the_stream_closes() {
        let harness = Harness::start("echo bye; exit 7", DEFAULT_BUFFER_BYTES).await;
        let mut link = harness.link().await;
        assert_eq!(next(&mut link).await, Output::Line("bye".into()));

        // EOF on the stream is the client's signal, and the contract is that the
        // tombstone is already there when it arrives.
        assert!(
            tokio::time::timeout(Duration::from_secs(5), link.recv())
                .await
                .expect("the shim closes the stream")
                .is_none(),
            "the stream ends when claude does"
        );
        let exit = harness.sidecars.read_tombstone().expect("the tombstone is written before the half-close");
        assert_eq!(exit.status, Some(7));
        assert_eq!(exit.signal, None);
        assert!(!exit.at.is_empty());
    }

    #[tokio::test]
    async fn claude_s_last_words_are_kept_and_the_socket_is_not() {
        let mut harness = Harness::start("echo 'it all went wrong' >&2; exit 3", DEFAULT_BUFFER_BYTES).await;
        // Attach, so the linger has somebody to drain to and ends promptly.
        let link = harness.link().await;
        let exit = harness.wait_for_tombstone().await;
        assert_eq!(exit.status, Some(3));
        drop(link);

        let shim = tokio::time::timeout(Duration::from_secs(20), &mut harness.shim)
            .await
            .expect("the shim exits after its linger")
            .expect("the shim task");
        assert_eq!(shim, Ok(()));
        assert_eq!(
            harness.sidecars.read_stderr().as_deref(),
            Some("it all went wrong\n"),
            "the stderr tail is the only account of a crash that survives"
        );
        assert!(!harness.sidecars.socket().exists(), "the socket goes with the shim");
    }

    #[tokio::test]
    async fn closing_stdin_ends_the_session_and_a_disconnect_does_not() {
        let harness = Harness::start("exec /bin/cat", DEFAULT_BUFFER_BYTES).await;
        let mut link = harness.link().await;
        link.send_line("still here").await.expect("send");
        assert_eq!(next(&mut link).await, Output::Line("still here".into()));

        link.close_stdin().await.expect("close");
        // `cat` exits when its stdin does, which is exactly what claude does.
        assert!(tokio::time::timeout(Duration::from_secs(5), link.recv())
            .await
            .expect("the stream closes")
            .is_none());
        assert_eq!(harness.wait_for_tombstone().await.status, Some(0), "a clean exit, not a killed one");
    }

    // ------------------------------------------------------- the corpse guard

    #[tokio::test]
    async fn attaching_to_a_shim_that_has_buried_its_claude_is_refused() {
        let harness = Harness::start("exit 0", DEFAULT_BUFFER_BYTES).await;
        harness.wait_for_tombstone().await;

        // The socket is still up — the shim lingers for a client that might yet
        // want the tail — so a plain connect succeeds and must not be trusted.
        let raw = connect(harness.sidecars.socket(), Duration::from_secs(5)).await;
        assert!(raw.is_ok(), "the corpse still answers: {raw:?}");
        drop(raw);

        let refused = attach(&harness.sidecars).await;
        assert!(
            matches!(refused, Err(LinkError::Corpse(_))),
            "a tombstone means a fresh spawn, not an attach: {refused:?}"
        );
    }

    #[tokio::test]
    async fn attaching_to_nothing_at_all_is_an_error_rather_than_a_hang() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sidecars = Sidecars::new(dir.path(), "0123456789abcdef");
        let error = attach(&sidecars).await.expect_err("there is no shim");
        assert!(matches!(error, LinkError::Connect(_)), "{error:?}");
    }

    #[tokio::test]
    async fn a_stale_tombstone_is_cleared_before_a_new_shim_can_be_condemned_by_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sidecars = Sidecars::new(dir.path(), "0123456789abcdef");
        sidecars.prepare().expect("prepare");
        std::fs::write(sidecars.exit(), b"{\"status\":9,\"signal\":null,\"at\":\"then\"}").expect("a corpse from last time");
        assert!(sidecars.tombstoned());
        sidecars.prepare().expect("prepare again");
        assert!(!sidecars.tombstoned(), "yesterday's exit must not condemn today's claude");
    }

    // ------------------------------------------------------------- the client

    #[test]
    fn a_gap_line_is_recognised_and_everything_else_is_claude_s() {
        assert_eq!(classify("{\"shim_gap\":{\"from\":1,\"to\":9}}"), Output::Gap { from: 1, to: 9 });
        // A line of claude's that merely mentions the key is still claude's.
        let quoted = "{\"type\":\"assistant\",\"text\":\"{\\\"shim_gap\\\":1}\"}";
        assert_eq!(classify(quoted), Output::Line(quoted.to_string()));
        assert_eq!(classify(""), Output::Line(String::new()));
    }

    #[test]
    fn the_sidecars_agree_on_a_directory_and_keep_the_socket_addressable() {
        let sidecars = Sidecars::new("/tmp/shims", "0123456789abcdef");
        assert_eq!(sidecars.exit(), PathBuf::from("/tmp/shims/0123456789abcdef.exit"));
        assert_eq!(sidecars.stderr(), PathBuf::from("/tmp/shims/0123456789abcdef.stderr"));
        assert_eq!(sidecars.socket(), Path::new("/tmp/shims/0123456789abcdef.sock"));

        // The real directory is far longer than a `sun_path`, and the socket has
        // to move rather than fail to bind. See `crate::pty_shim::sidecar`.
        let deep = PathBuf::from("/Users/somebody/.local/share/remote-hand/instances/production-instance/session-shims");
        let moved = Sidecars::new(&deep, "9f8e7d6c5b4a39281706");
        assert!(
            moved.socket().to_string_lossy().starts_with("/tmp/remote-hand-"),
            "the address does not fit beside the other files: {}",
            moved.socket().display()
        );
        assert!(moved.exit().starts_with(&deep), "the files that say what a session is never move");
    }
    /// The session shim command lines, pinned so the hand parser and clap answer
    /// alike. `--buffer-bytes` is the undocumented test knob; a bad number is
    /// ignored, as every shim number is.
    #[test]
    fn the_session_shim_argument_table() {
        let words = |line: &[&str]| -> Vec<String> { line.iter().map(|word| (*word).to_string()).collect() };
        let accepted: Vec<Vec<String>> = [
            &["--socket", "/s", "--stderr-file", "/err", "--exit-file", "/e", "--", "/bin/claude"][..],
            &["--exit-file", "/e", "--stderr-file", "/err", "--socket", "/s", "--", "/bin/claude", "--flag"],
            &["--socket", "/s", "--stderr-file", "/err", "--exit-file", "/e", "--buffer-bytes", "1024", "--", "/bin/claude"],
            &["--socket", "/s", "--stderr-file", "/err", "--exit-file", "/e", "--buffer-bytes", "x", "--", "/bin/claude"],
            &["--socket", "/s", "--stderr-file", "/err", "--exit-file", "/e", "--buffer-bytes", "1", "--buffer-bytes", "x", "--", "/bin/claude"],
            &["--socket", "--", "--stderr-file", "/err", "--exit-file", "/e", "--", "/bin/claude"],
        ]
        .into_iter()
        .map(words)
        .collect();
        for argv in &accepted {
            assert!(parse(argv).is_ok(), "{argv:?} must parse: {:?}", parse(argv).err());
        }
        let parsed = parse(&words(&["--socket", "/s", "--stderr-file", "/err", "--exit-file", "/e", "--buffer-bytes", "1024", "--", "/bin/claude"])).expect("parsed");
        assert_eq!(parsed.buffer_bytes, 1024);
        let parsed = parse(&words(&["--socket", "/s", "--stderr-file", "/err", "--exit-file", "/e", "--buffer-bytes", "x", "--", "/bin/claude"])).expect("parsed");
        assert_eq!(parsed.buffer_bytes, DEFAULT_BUFFER_BYTES);

        let refused: Vec<Vec<String>> = [
            &[][..],
            &["--socket", "/s", "--stderr-file", "/err", "--exit-file", "/e"],
            &["--socket", "/s", "--stderr-file", "/err", "--exit-file", "/e", "--"],
            &["--socket", "/s", "--stderr-file", "/err", "--exit-file", "/e", "/bin/claude"],
            &["--socket=/s", "--stderr-file", "/err", "--exit-file", "/e", "--", "/bin/claude"],
            &["--socket", "/s", "--stderr-file=/err", "--exit-file", "/e", "--", "/bin/claude"],
            &["--exit-file", "/e", "--stderr-file", "/err", "--", "/bin/claude"],
            &["--socket", "/s", "--exit-file", "/e", "--", "/bin/claude"],
            &["--help"],
        ]
        .into_iter()
        .map(words)
        .collect();
        for argv in &refused {
            assert!(parse(argv).is_err(), "{argv:?} must be refused");
        }
        assert_eq!(parse(&words(&[])).unwrap_err(), "session-shim: --socket is required");
        assert_eq!(parse(&words(&["--socket", "/s"])).unwrap_err(), "session-shim: --stderr-file is required");
        assert_eq!(
            parse(&words(&["--socket", "/s", "--stderr-file", "/err"])).unwrap_err(),
            "session-shim: --exit-file is required"
        );
        assert_eq!(
            parse(&words(&["--socket", "/s", "--stderr-file", "/err", "--exit-file", "/e"])).unwrap_err(),
            "session-shim: the command must follow `--`"
        );
    }

}
