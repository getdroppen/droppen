//! The shim process: `remote-hand-server pty-shim --socket … --exit-file … -- <shell> …`.
//!
//! One shim owns one pty and one shell. It is spawned detached and in its own
//! process group, so when the server that started it exits — cleanly, on
//! `SIGKILL`, or because it was upgraded — the shim, the pty and the shell are
//! all still there. A new server finds the socket and reattaches.
//!
//! That is the whole feature, and it is why almost everything here is about
//! *order*:
//!
//! * the slave fd is dropped immediately after the spawn, so the master reaches
//!   EOF when the shell dies rather than when the shim feels like it;
//! * the tombstone is written **before** the `x` frame goes out, so a server
//!   that dies while reading the frame still learns the shell exited;
//! * the socket is unlinked **last**, and only after checking the path still
//!   points at the socket this process bound — a successor may already have
//!   bound a fresh one there.
//!
//! The shim never interprets the stream. It records bytes and it repaints
//! screens; deciding what the browser sees is the server's job.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::Parser;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtyPair, PtySize};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch};

use super::proto::{self, Exit, Frame, Hello, Hello2, Resize, Tombstone};
use super::ring::{Parsed, Recorder, DEFAULT_COLS, DEFAULT_ROWS};

/// How long the shim waits for clients to hang up after announcing the exit.
const LINGER: Duration = Duration::from_secs(15);
/// How long the reader is given to drain a dead shell's last output before the
/// exit is announced anyway and the reader is woken by force. A grandchild
/// holding the tty open would otherwise keep it — and the whole shim — alive
/// indefinitely.
const DRAIN_GRACE: Duration = Duration::from_millis(500);
/// How often the linger loop checks whether the last client has gone.
const LINGER_POLL: Duration = Duration::from_millis(25);
/// One read from the master fd.
const READ_CHUNK: usize = 64 * 1024;
/// How many times a dead accept loop — or a dead reader thread — is restarted
/// before the shim gives up. One budget shape for both, because they fail the
/// same way and cost the same thing: see [`keep_serving`].
const ACCEPT_RESTARTS: u32 = 3;
/// How long a restarted accept loop or reader must survive to count as recovered
/// rather than as one more turn of a spin. One that dies instantly, forever, must
/// not keep this process alive at full tilt doing nothing.
const ACCEPT_STABLE: Duration = Duration::from_secs(10);
/// How many lines one terminal's fault file will ever hold. See [`Faults`].
const MAX_FAULT_NOTES: usize = 16;
/// The largest output frame the shim will emit in one go.
const MAX_OUTPUT_FRAME: usize = 256 * 1024;
/// The largest repaint frame the shim will emit in one go.
///
/// The same size as an output frame, for the same reason and one more. A repaint
/// is the one thing the shim sends whose size is set by the *window* rather than
/// by how much the shell wrote, and a window large enough — a thousand columns by
/// a thousand rows, every cell carrying its own colour — repaints into more than
/// the [`proto::MAX_PAYLOAD`] the peer will accept. That is not a slow reattach
/// but a permanent one: the receiver refuses the length prefix, the link drops,
/// the client dials again and meets the same wall for as long as the window is
/// that size. So a repaint is split, and [`snapshot_frames`] does the splitting.
const MAX_SNAPSHOT_FRAME: usize = MAX_OUTPUT_FRAME;

/// What the shim was told to do.
#[derive(Debug, Clone)]
pub struct Args {
    /// Where to bind the unix socket.
    pub socket: PathBuf,
    /// Where to write the tombstone.
    pub exit_file: PathBuf,
    /// The program to run under the pty.
    pub command: String,
    /// Its arguments, `argv[0]` excluded.
    pub args: Vec<String>,
    /// The pty's size, if the server asked for one.
    ///
    /// Given at `openpty` rather than sent as a resize afterwards, because the
    /// child reads its width the moment it starts: a terminal widened a
    /// millisecond later has already wrapped whatever it printed first. The
    /// account wizards depend on that ordering — see `accounts::setup_token`.
    pub size: Option<(u16, u16)>,
}

/// Parse everything after the `pty-shim` subcommand.
///
/// `--` is mandatory and is what separates the shim's own flags from the
/// command. The separator is not a nicety: a command line assembled without one
/// eventually gets a flag-shaped argument through to the wrong parser.
pub fn parse(argv: &[String]) -> Result<Args, String> {
    if let Some(token) = refused_spelling(argv) {
        return Err(format!("pty-shim: unexpected argument {token:?}"));
    }
    let cli = PtyShimCli::try_parse_from(std::iter::once("pty-shim".to_string()).chain(argv.iter().cloned()))
        .map_err(|error| error.to_string())?;
    let socket = cli.socket.ok_or_else(|| "pty-shim: --socket is required".to_string())?;
    let exit_file = cli.exit_file.ok_or_else(|| "pty-shim: --exit-file is required".to_string())?;
    // Required by hand, in the old order and with the old words: clap would
    // name the missing command first and say it differently.
    let mut rest = cli.command;
    if rest.is_empty() {
        return Err("pty-shim: the command must follow `--`".to_string());
    }
    let command = rest.remove(0);
    Ok(Args {
        socket: PathBuf::from(socket),
        exit_file: PathBuf::from(exit_file),
        command,
        args: rest,
        // Both or neither: half a size is not a size. A value the old parser
        // could not read as a number is the same as one that was not given.
        size: cli
            .cols
            .and_then(|value| value.parse::<u16>().ok())
            .zip(cli.rows.and_then(|value| value.parse::<u16>().ok()))
            .filter(|(cols, rows)| *cols > 0 && *rows > 0),
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
            "--socket" | "--exit-file" | "--cols" | "--rows" => {
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
    ["--socket=", "--exit-file=", "--cols=", "--rows="]
        .iter()
        .any(|prefix| arg.starts_with(prefix))
}

/// `pty-shim`'s command line: its flags, then the mandatory `--` and the
/// command it runs. `last` is what makes the separator mandatory and keeps
/// everything after it out of clap's flag parsing.
#[derive(Debug, Parser)]
#[command(
    name = "pty-shim",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    args_override_self = true
)]
struct PtyShimCli {
    #[arg(long, allow_hyphen_values = true)]
    socket: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    exit_file: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    cols: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    rows: Option<String>,
    #[arg(last = true, num_args = 0..)]
    command: Vec<String>,
}

/// What a connection's mixer task is told to do.
enum Command {
    /// A frame the client sent.
    Frame(Frame),
    /// The shell exited; drain, announce, half-close.
    Exit(i32),
}

/// Everything one shim shares between its tasks and threads.
struct Shim {
    recorder: Arc<Mutex<Recorder>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    input: std::sync::mpsc::Sender<Vec<u8>>,
    shell_pid: u32,
    /// Bumped every time the recorder grows, so mixers can wait rather than poll.
    total: watch::Sender<u64>,
    clients: Arc<Mutex<Vec<mpsc::UnboundedSender<Command>>>>,
    /// How many client sockets are still open, for the linger.
    live: Arc<AtomicUsize>,
    /// Whether anybody has ever attached.
    ///
    /// Without it, a shell that exits before its own server has finished
    /// connecting — `/usr/bin/true` is the everyday case — would find zero
    /// clients, linger for zero seconds and unlink a socket the server is at
    /// that moment dialling.
    attached: Arc<AtomicBool>,
    /// The shell's exit status once it has one, so a client that arrives *after*
    /// the exit is told at hello rather than left waiting for output that will
    /// never come. [`NOT_EXITED`] means it is still running.
    exit_code: Arc<AtomicI64>,
    /// Where this shim writes down what went wrong with it.
    faults: Faults,
}

/// The [`Shim::exit_code`] sentinel for "still running".
const NOT_EXITED: i64 = i64::MIN;

impl Shim {
    /// Send a command to every attached client, dropping the ones that are gone.
    fn broadcast(&self, make: impl Fn() -> Command) {
        // Recovered rather than propagated, for the reason [`Shim::recorder`]
        // gives at length: this is called from the shutdown contract, and a
        // panic here would skip the exit frame every attached client is waiting
        // for. Nothing that can panic runs under this lock today — which is an
        // argument for it never being poisoned, not for handling it badly if it
        // is.
        let mut clients = self.clients.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        clients.retain(|client| client.send(make()).is_ok());
    }

    /// The recorder, taken back from a panic rather than propagating it.
    ///
    /// The house pattern, for the house reason — `crate::db::Db::lock` states
    /// it in full. A `vt100` parse that panicked inside [`read_pty`] *while it
    /// held this lock* would poison it: from then on every one of the five
    /// places that take it would panic in turn, the worst being the `HELLO` arm
    /// of [`Shim::mix`], which takes it one line before it writes the reply — so
    /// every client that dialled would have its task killed before a single
    /// byte went out, and nothing would get past the handshake, while the
    /// socket kept accepting connections the shim never answered.
    ///
    /// What is behind the poison is usable — [`Recorder::append`] rebuilds
    /// whatever the panic left — and even if it were not, a stale screen is a
    /// far smaller thing than a terminal nobody can attach to.
    fn recorder(&self) -> std::sync::MutexGuard<'_, Recorder> {
        self.recorder.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Where the shim writes down something that should never have happened.
///
/// Not a log, because a shim has nowhere to log to: it installs no tracing
/// subscriber of its own, so the `tracing::error!` lines in this file are
/// collected only when something else in the process has installed one — a test
/// harness, or a shim that one day grows one. In production they go nowhere, so
/// the few lines that mean *this terminal is broken* are also written here, as
/// `<id>.fault` beside the tombstone.
///
/// Two deliberate properties:
///
/// * it outlives the terminal. `Sidecars::remove_all` does not know this file,
///   so the question it exists to answer — why did that terminal die at three
///   in the morning — can still be asked the next day. Nothing adopts it
///   either: `sidecar::scan` matches three suffixes and this is not one;
/// * it is bounded to [`MAX_FAULT_NOTES`] lines, so a fault that repeats
///   forever cannot become the largest file in the instance directory.
struct Faults {
    path: PathBuf,
    written: AtomicUsize,
}

impl Faults {
    /// The fault file beside a terminal's tombstone: `<id>.exit` → `<id>.fault`.
    fn beside(exit_file: &Path) -> Self {
        Self {
            path: exit_file.with_extension("fault"),
            written: AtomicUsize::new(0),
        }
    }

    /// Write one line, if there is still room for one.
    ///
    /// Failures are dropped rather than reported: this is already the error
    /// path, and a shim that could not describe its own fault still has a
    /// tombstone to write and clients to hang up on.
    fn note(&self, what: &str) {
        if self.written.fetch_add(1, Ordering::SeqCst) >= MAX_FAULT_NOTES {
            return;
        }
        use std::io::Write as _;
        let at = crate::sessions::iso8601(std::time::SystemTime::now());
        let line = format!("{at} pid={} {what}\n", std::process::id());
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut file| file.write_all(line.as_bytes()));
        // And to stderr, wherever the server has pointed it. A panic's own
        // message and location arrive there through the default hook, and this
        // is the line that says what the shim then did about it; the two are
        // worth an operator's time in that order, in one place. Costs a write
        // to `/dev/null` when nobody kept the stream.
        eprint!("{line}");
    }
}

/// Run one shim to completion. Returns when the shell is gone and the socket is
/// cleaned up.
pub async fn run(args: Args) -> Result<(), String> {
    let (cols, rows) = args.size.unwrap_or((DEFAULT_COLS, DEFAULT_ROWS));
    let PtyPair { slave, master } = native_pty_system()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("pty-shim: cannot open a pty: {error}"))?;

    // The environment and working directory are the server's, inherited: it
    // scrubbed them once, and a second copy of that policy here could disagree
    // with it after an upgrade.
    let mut command = CommandBuilder::new(&args.command);
    command.args(&args.args);
    if let Ok(cwd) = std::env::current_dir() {
        command.cwd(cwd);
    }
    let mut child = slave
        .spawn_command(command)
        .map_err(|error| format!("pty-shim: cannot spawn {:?}: {error}", args.command))?;
    // `portable-pty` puts the child in a session of its own, so its pid is also
    // its process group id — which is what `k` signals.
    let shell_pid = child.process_id().unwrap_or(0);
    // The master only reaches EOF once every slave handle is closed, and this
    // process holding one would mean it never does.
    drop(slave);

    let master_fd = master.as_raw_fd().ok_or_else(|| "pty-shim: the master has no descriptor".to_string())?;
    let writer = master
        .take_writer()
        .map_err(|error| format!("pty-shim: cannot write to the pty: {error}"))?;

    // Bind before anything else can be told about this terminal: the server
    // connects the moment it returns from spawning us.
    let _ = std::fs::remove_file(&args.socket);
    let listener = UnixListener::bind(&args.socket).map_err(|error| format!("pty-shim: cannot bind {}: {error}", args.socket.display()))?;
    // Recorded now, compared at unlink time: see `unlink_if_ours`.
    let identity = path_identity(&args.socket);

    let (input, input_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let (total, _) = watch::channel(0u64);
    let shim = Arc::new(Shim {
        recorder: Arc::new(Mutex::new(Recorder::new(rows, cols))),
        master: Arc::new(Mutex::new(master)),
        input,
        shell_pid,
        total,
        clients: Arc::new(Mutex::new(Vec::new())),
        live: Arc::new(AtomicUsize::new(0)),
        attached: Arc::new(AtomicBool::new(false)),
        exit_code: Arc::new(AtomicI64::new(NOT_EXITED)),
        faults: Faults::beside(&args.exit_file),
    });

    spawn_writer(writer, input_rx);
    let mut reader = spawn_reader(master_fd, shim.clone())?;

    let (exited_tx, exited_rx) = tokio::sync::oneshot::channel::<i32>();
    std::thread::spawn(move || {
        let code = child.wait().map(|status| status.exit_code() as i32).unwrap_or(-1);
        let _ = exited_tx.send(code);
    });

    // Accepting runs for the shim's whole life, the linger included: a server
    // that connects *after* the shell died still has to be told that it did.
    // And it is *watched* for the shim's whole life, along with the reader,
    // because a shim that has stopped doing either looks exactly like one that
    // has not — see [`keep_serving`].
    let listener = Arc::new(listener);
    let mut accepting = tokio::spawn(accept_loop(listener.clone(), shim.clone()));
    // The id the server knows this terminal by. The tombstone is named after
    // it, so it is what turns the log lines below into something anybody can
    // match against a terminal that misbehaved.
    let id = args.exit_file.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let respawn_accept = {
        let listener = listener.clone();
        let shim = shim.clone();
        move || tokio::spawn(accept_loop(listener.clone(), shim.clone()))
    };
    let respawn_reader = {
        let shim = shim.clone();
        // The master outlives every reader — it is owned by `shim` — so a fresh
        // `dup` of it is all a new thread needs.
        move || spawn_reader(master_fd, shim.clone())
    };
    let code = match keep_serving(exited_rx, &mut accepting, respawn_accept, &mut reader, respawn_reader, &shim.faults, &id).await {
        Ending::Shell(code) => code,
        // The shim can no longer do the one thing it exists for, so it stops
        // claiming to be a terminal: the exit path below writes the tombstone,
        // which is what makes the next boot sweep it (`Registry::readopt_one`)
        // instead of adopting a socket that will never answer, or a screen that
        // will never change again. The status is the one a shell whose end was
        // never observed already gets.
        //
        // The shell may still be alive, and it is deliberately not signalled:
        // when this process exits, the pty master closes with it and the kernel
        // hangs up the session, which is the same death a closed terminal
        // window is. Losing an unreachable shell that way is the cheaper half
        // of the trade. The other half is what a shim that stays up around an
        // idle shell shows the browser: a snapshot from its last adoption and
        // every keystroke swallowed, with no way out but killing it by hand.
        Ending::Deaf | Ending::Blind => -1,
    };

    // Order from here to the end of the function is the contract. Nothing below
    // may be reordered without breaking a restart that lands in the middle of it.

    // 1. The tombstone, first, because it is the only record a server that is
    //    not attached — or that dies mid-frame — will ever see.
    write_tombstone(&args.exit_file, code);

    // 2. Then, once the reader has recorded the shell's last output, the
    //    status a late client is told at hello and the exit every attached
    //    one is told through its mixer. See [`announce_exit`].
    shim.announce_exit(&mut reader, code, DRAIN_GRACE).await;

    // 3. The reader is the one thread that can outlive the shell: it sits in
    //    `poll(2)` on a master a grandchild may still hold open. It has had its
    //    grace already, inside the announcement.
    let _ = tokio::task::spawn_blocking(move || reader.stop()).await;

    // 4. Linger, so a client that is mid-read gets the exit frame — and so a
    //    server that has not managed to connect yet still can.
    let deadline = std::time::Instant::now() + LINGER;
    while std::time::Instant::now() < deadline {
        let waiting = shim.live.load(Ordering::SeqCst) > 0 || !shim.attached.load(Ordering::SeqCst);
        if !waiting {
            break;
        }
        tokio::time::sleep(LINGER_POLL).await;
    }

    // 5. And only now the socket — guarded, because by this point a successor
    //    terminal may have bound a fresh one at the same path.
    accepting.abort();
    drop(listener);
    unlink_if_ours(&args.socket, identity);
    Ok(())
}

impl Shim {
    /// Tell every client the shell exited, but only after the reader has
    /// recorded the last of what it wrote — or `grace` has passed.
    ///
    /// The shell's status and its last output reach the shim by two roads that
    /// do not wait for each other: `waitpid` on one thread, `read(2)` on the
    /// master on another. A CLI that prints and exits at once — `claude
    /// setup-token` printing its token is exactly that — can have its status in
    /// hand, on a busy machine, while the reader has not yet read the final
    /// write. Announcing then sent every client the exit frame with the token
    /// still in the pty buffer, and since a mixer stops at the exit frame, the
    /// token never went out at all: the wizard saw a CLI that exited without
    /// printing one.
    ///
    /// So the reader is waited for first. It ends at EOF, which a pty master
    /// only reaches once its buffer is drained and every slave handle is
    /// closed, so everything the shell wrote is in the recorder by then and the
    /// mixer's last pump carries it ahead of the exit frame. `grace` bounds the
    /// wait for the case EOF never comes, a backgrounded grandchild still
    /// holding the tty.
    ///
    /// The exit code is published after the wait too, not before: a client
    /// that says hello during it would otherwise be handed the backlog so far
    /// and the exit straight after, short of the same final write.
    async fn announce_exit(&self, reader: &mut Reader, code: i32, grace: Duration) {
        // A reader that already ended has dropped its report sender, so this
        // returns at once whether or not `keep_serving` took the report.
        let _ = tokio::time::timeout(grace, reader.ended.recv()).await;
        self.exit_code.store(i64::from(code), Ordering::SeqCst);
        self.broadcast(|| Command::Exit(code));
    }
}

/// Which of the things the shim waits on happened first.
enum Ending {
    /// The shell exited, with this status.
    Shell(i32),
    /// The accept loop would not stay up. Nothing can reach this shim again.
    Deaf,
    /// The reader thread would not stay up. Nothing is reading the pty, so the
    /// terminal stays frozen at whatever it last recorded.
    Blind,
}

/// One supervised thing's restart budget.
struct Budget {
    restarts: u32,
    started: std::time::Instant,
}

impl Budget {
    fn new() -> Self {
        Self {
            restarts: 0,
            started: std::time::Instant::now(),
        }
    }

    /// Record a failure and take a restart, or `None` once the budget is spent.
    fn take(&mut self) -> Option<u32> {
        // Something that served for a while before dying is a fresh failure
        // rather than the same one repeating, so it does not spend the budget a
        // spin is meant to catch.
        if self.started.elapsed() >= ACCEPT_STABLE {
            self.restarts = 0;
        }
        self.restarts += 1;
        self.started = std::time::Instant::now();
        (self.restarts <= ACCEPT_RESTARTS).then_some(self.restarts)
    }
}

/// Wait for the shell, keeping both of the shim's lifelines alive meanwhile.
///
/// Awaiting only the shell would miss both failures this prevents, and an idle
/// login shell can outlast the loss of either by days.
///
/// **Accepting.** [`accept_loop`] has no `break`, so the only thing that ends it
/// is a panic in its task — and a panic there is silent in every way that
/// matters: `main` still holds its own handle on the listener, so the socket
/// stays bound, `connect(2)` still succeeds and the kernel still completes the
/// connection into the backlog. Nobody ever accepts it, so the shim never
/// speaks, so no client ever gets past hello.
///
/// **Reading.** [`read_pty`] sits in `poll(2)` for as long as the shell lives,
/// so it can only leave that loop by being woken deliberately, by the pty
/// closing, or by a panic. A shim that has lost this one has nothing in
/// `read_pty`: nothing is reading the master, so the recorder never grows and no
/// client has anything to be sent — while the shell sits there perfectly
/// healthy, filling a pty buffer nobody will ever drain, and the browser shows
/// the snapshot from its last adoption and swallows every keystroke.
///
/// Either one that ends is restarted — the shell is fine and the user's terminal
/// should keep working — and logged at `error`, because what panicked is still
/// unknown and that line, with the [`Faults`] note beside it, is the only thing
/// the next occurrence will leave behind. The restarts are bounded so that
/// something which dies instantly cannot spin here forever; past the bound the
/// caller takes the exit path, which is honest about what the shim has become.
async fn keep_serving(
    mut exited: tokio::sync::oneshot::Receiver<i32>,
    accepting: &mut tokio::task::JoinHandle<()>,
    mut respawn_accept: impl FnMut() -> tokio::task::JoinHandle<()>,
    reader: &mut Reader,
    mut respawn_reader: impl FnMut() -> Result<Reader, String>,
    faults: &Faults,
    id: &str,
) -> Ending {
    let mut accept_budget = Budget::new();
    let mut reader_budget = Budget::new();
    // The reader is watched until the pty is finished with, and not after: EOF
    // means every handle on the slave has been closed, which is the shell and
    // everything it started being gone, so a fresh reader would meet the same
    // EOF while the shell's own status is already on its way here.
    let mut reading = true;
    loop {
        tokio::select! {
            code = &mut exited => return Ending::Shell(code.unwrap_or(-1)),
            ended = &mut *accepting => {
                let Some(restarts) = accept_budget.take() else {
                    let what = "accepting will not stay up; shutting the terminal down rather than holding a socket nobody answers on";
                    tracing::error!(%id, restarts = ACCEPT_RESTARTS, "pty-shim: {what}");
                    faults.note(what);
                    return Ending::Deaf;
                };
                let what = "the accept loop ended and only this restarts it; a shim that stops accepting looks alive and answers nothing";
                tracing::error!(%id, restarts, outcome = ?ended, "pty-shim: {what}");
                faults.note(&format!("restart {restarts}: {what} (outcome {ended:?})"));
                *accepting = respawn_accept();
            }
            end = reader.ended.recv(), if reading => {
                if end == Some(ReaderEnd::Closed) {
                    reading = false;
                    continue;
                }
                // `None` is the report channel's sender going away with a thread
                // that never said why, which is not a healthy end either.
                let Some(restarts) = reader_budget.take() else {
                    let what = "the pty reader will not stay up; shutting the terminal down rather than showing a screen that can never change";
                    tracing::error!(%id, restarts = ACCEPT_RESTARTS, "pty-shim: {what}");
                    faults.note(what);
                    return Ending::Blind;
                };
                // Whatever the dead thread had already read and not yet recorded
                // is gone, and no client is told a number for it: the ring
                // counts what it recorded, so every offset still names the byte
                // it always named. The output between the two readers is simply
                // absent, the way output written while nothing was attached is.
                let what = "the pty reader ended and only this restarts it; nothing else in this process reads the master";
                tracing::error!(%id, restarts, ?end, "pty-shim: {what}");
                faults.note(&format!("restart {restarts}: {what} (end {end:?})"));
                match respawn_reader() {
                    Ok(fresh) => *reader = fresh,
                    Err(error) => {
                        tracing::error!(%id, %error, "pty-shim: the pty reader cannot be started again");
                        faults.note(&format!("the pty reader cannot be started again: {error}"));
                        return Ending::Blind;
                    }
                }
            }
        }
    }
}

/// Accept clients for as long as the shim lives.
async fn accept_loop(listener: Arc<UnixListener>, shim: Arc<Shim>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => serve_client(stream, shim.clone()),
            Err(error) => {
                tracing::warn!(%error, "pty-shim: accept failed");
                // A listener that cannot accept will not start again on its own;
                // the shell is still fine, so keep it and let the next boot
                // decide what to do about it.
                tokio::time::sleep(LINGER_POLL).await;
            }
        }
    }
}

/// Wire up one client: a socket reader, a mixer and a socket writer.
///
/// Three tasks rather than one because a framed read is not cancellation-safe:
/// polling it inside the mixer's `select!` would eat a frame every time the
/// other arm won. The reader owns the read half and does nothing but hand whole
/// frames on.
fn serve_client(stream: UnixStream, shim: Arc<Shim>) {
    let (mut rd, mut wr) = stream.into_split();
    let (commands, command_rx) = mpsc::unbounded_channel::<Command>();
    let (frames, mut frame_rx) = mpsc::unbounded_channel::<Option<Frame>>();

    // Recovered rather than `expect`ed, like [`Shim::recorder`] and for the
    // neighbouring reason: this one is taken *inside* [`accept_loop`], where a
    // panic does not end a client but the shim's whole ability to accept one,
    // and leaves the socket bound and unserved for as long as the shell lives.
    // What the lock guards
    // is a list of senders with no invariant a panic could have left half
    // built, so taking it back is both safe and better than dying.
    shim.clients
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(commands.clone());
    shim.live.fetch_add(1, Ordering::SeqCst);
    shim.attached.store(true, Ordering::SeqCst);

    // Writer: frames out, and `None` to half-close.
    tokio::spawn(async move {
        while let Some(frame) = frame_rx.recv().await {
            match frame {
                Some(frame) => {
                    if proto::write_frame(&mut wr, &frame).await.is_err() {
                        break;
                    }
                }
                // Half-close: the client sees EOF, we still read whatever it has
                // left to say.
                None => break,
            }
        }
        drop(wr);
    });

    // Reader: whole frames in.
    let live = shim.live.clone();
    tokio::spawn(async move {
        loop {
            match proto::read_frame(&mut rd).await {
                Ok(Some(frame)) => {
                    if commands.send(Command::Frame(frame)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    tracing::debug!(%error, "pty-shim: client link ended");
                    break;
                }
            }
        }
        live.fetch_sub(1, Ordering::SeqCst);
    });

    tokio::spawn(shim.mix(command_rx, frames));
}

impl Shim {
    /// One client's state machine: the only place its cursor is read or
    /// written.
    ///
    /// Keeping the cursor here — rather than in a separate output pump — is
    /// what makes the reattach sequence honest. A snapshot and the output it
    /// already reflects are taken under one lock, output first, so the client
    /// can never be shown a repaint and then the bytes that repaint already
    /// contained.
    async fn mix(self: Arc<Self>, mut commands: mpsc::UnboundedReceiver<Command>, out: mpsc::UnboundedSender<Option<Frame>>) {
        let mut total = self.total.subscribe();
        let mut cursor = 0u64;
        let mut attached = false;

        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(Command::Frame(frame)) => match frame.kind {
                        proto::HELLO => {
                            let Some(hello) = frame.parse::<Hello>() else { continue };
                            let (resume, gap, live) = {
                                let recorder = self.recorder();
                                let ring = recorder.ring();
                                let (resume, gap) = ring.resume_at(hello.cursor);
                                (resume, gap, ring.total())
                            };
                            if out.send(Some(Frame::json(proto::HELLO, &Hello2 { shell_pid: self.shell_pid, total: live }))).is_err() {
                                return;
                            }
                            // The gap goes out before the first byte of output, so
                            // the client can render it in the right place.
                            if let Some(gap) = gap {
                                if out.send(Some(Frame::json(proto::GAP, &gap))).is_err() {
                                    return;
                                }
                            }
                            cursor = resume;
                            attached = true;
                            if !self.pump(&mut cursor, &out) {
                                return;
                            }
                            // A client that arrived after the shell died gets the
                            // backlog and then the news, in that order.
                            let code = self.exit_code.load(Ordering::SeqCst);
                            if code != NOT_EXITED {
                                let _ = out.send(Some(Frame::json(proto::EXIT, &Exit { code: code as i32 })));
                                let _ = out.send(None);
                                return;
                            }
                        }
                        proto::INPUT => {
                            let _ = self.input.send(frame.payload);
                        }
                        proto::RESIZE => {
                            let Some(size) = frame.parse::<Resize>() else { continue };
                            self.resize(size);
                        }
                        proto::SNAPSHOT => {
                            // Output first, then the repaint, both from one lock.
                            let (pending, snapshot, live) = {
                                let recorder = self.recorder();
                                (recorder.ring().read_from(cursor), recorder.snapshot(), recorder.ring().total())
                            };
                            if !out.send_output(pending) {
                                return;
                            }
                            cursor = live;
                            if !out.send_snapshot(&snapshot) {
                                return;
                            }
                        }
                        proto::KILL => terminate_shell(self.shell_pid),
                        _ => {}
                    },
                    Some(Command::Exit(code)) => {
                        // A client that has not said hello is owed a hello first,
                        // and the server's handshake reads nothing else: the exit
                        // frame in its place fails the handshake, the client hangs
                        // up, and a shim whose only client has come and gone
                        // unlinks its socket at once, so every retry finds none.
                        // That is how a shell that exits as it starts (`true`,
                        // `echo hi`) lost its terminal to a server that dialled in
                        // the same instant. Nothing is lost by waiting: the exit
                        // code was published before this was broadcast, so the
                        // hello that follows is answered with the backlog and then
                        // the exit (see the `HELLO` arm).
                        if !attached {
                            continue;
                        }
                        if !self.pump(&mut cursor, &out) {
                            return;
                        }
                        let _ = out.send(Some(Frame::json(proto::EXIT, &Exit { code })));
                        let _ = out.send(None);
                        return;
                    }
                    None => return,
                },
                changed = total.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    if attached && !self.pump(&mut cursor, &out) {
                        return;
                    }
                }
            }
        }
    }
}

impl Shim {
    /// Send everything the client has not seen. `false` means the client is gone.
    fn pump(&self, cursor: &mut u64, out: &mpsc::UnboundedSender<Option<Frame>>) -> bool {
        let (pending, live) = {
            let recorder = self.recorder();
            (recorder.ring().read_from(*cursor), recorder.ring().total())
        };
        *cursor = live;
        out.send_output(pending)
    }
}

/// Sending frames to one client.
trait FrameSink {
    /// Frame output, chunked so one enormous backlog is not one enormous
    /// allocation on the far side.
    fn send_output(&self, bytes: Vec<u8>) -> bool;

    /// Send a repaint, split if it has to be. `false` means the client is gone.
    fn send_snapshot(&self, snapshot: &[u8]) -> bool;
}

impl FrameSink for mpsc::UnboundedSender<Option<Frame>> {
    fn send_output(&self, bytes: Vec<u8>) -> bool {
        if bytes.is_empty() {
            return true;
        }
        for chunk in bytes.chunks(MAX_OUTPUT_FRAME) {
            if self.send(Some(Frame::raw(proto::OUTPUT, chunk.to_vec()))).is_err() {
                return false;
            }
        }
        true
    }

    fn send_snapshot(&self, snapshot: &[u8]) -> bool {
        for frame in snapshot_frames(snapshot, MAX_SNAPSHOT_FRAME) {
            if self.send(Some(frame)).is_err() {
                return false;
            }
        }
        true
    }
}

/// Split a repaint into frames of at most `limit` bytes, never cutting through
/// the middle of a character.
///
/// Consecutive `s` frames concatenate: a repaint is escape sequences bound for a
/// terminal, and a terminal reads a byte stream, so where the pieces were cut
/// makes no difference to what it draws. Where it is cut matters for one reason
/// only — the server decodes each payload on its own before forwarding it, so a
/// character straddling two frames would reach the browser as a pair of
/// replacement characters and stay wrong until the next repaint.
///
/// Always at least one frame, including for an empty repaint: the server folds
/// output into the browser's stream until a repaint arrives, and a repaint that
/// is never sent is one it waits out its whole timeout for.
fn snapshot_frames(snapshot: &[u8], limit: usize) -> Vec<Frame> {
    let mut frames = Vec::new();
    let mut rest = snapshot;
    while rest.len() > limit {
        // `state_formatted` is UTF-8, so the boundary is at most three bytes
        // back; a `limit` smaller than one character is not a shape this is ever
        // called with, and cutting is still better than not terminating.
        let mut cut = limit;
        while cut > 0 && rest[cut] & 0b1100_0000 == 0b1000_0000 {
            cut -= 1;
        }
        if cut == 0 {
            cut = limit;
        }
        frames.push(Frame::raw(proto::SNAPSHOT, &rest[..cut]));
        rest = &rest[cut..];
    }
    frames.push(Frame::raw(proto::SNAPSHOT, rest));
    frames
}

impl Shim {
    /// Resize the pty and the parser that models it.
    fn resize(&self, size: Resize) {
        if size.cols == 0 || size.rows == 0 || size.cols >= 1000 || size.rows >= 1000 {
            return;
        }
        {
            // Same reasoning as the two locks above. A poisoned master costs a
            // resize, and a resize must never be the thing that takes a
            // terminal down.
            let master = self.master.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = master.resize(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
        if self.recorder().resize(size.rows, size.cols) == Parsed::Reset {
            // The same fault as in [`read_pty`], reached through the other door:
            // a resize is the only other thing that reaches into the parser.
            tracing::error!(?size, "pty-shim: the screen model was rebuilt after a resize that panicked");
            self.faults.note("the screen model was rebuilt after a resize that panicked");
        }
    }
}

/// How long a shell is given to take the hint before it is killed outright.
const KILL_ESCALATION: Duration = Duration::from_millis(750);

/// Signal the shell's process group.
///
/// A signal, never a `kill(1)`: shelling out puts a pid this process computed
/// into a string another program re-parses, and a missing `--` in that string
/// can signal every process owned by the uid. `libc::kill` cannot be talked into
/// that.
///
/// The `<= 1` guard is the same lesson in numeric form: a pid the shim failed to
/// learn is `0`, and `kill(-0, …)` is "my own process group", `kill(-1, …)` is
/// "every process I may signal". Both are the accident, not the intent.
pub fn kill_shell(shell_pid: u32, signal: libc::c_int) {
    let pgid = shell_pid as libc::pid_t;
    if pgid <= 1 {
        tracing::warn!(shell_pid, "refusing to signal an implausible process group");
        return;
    }
    // SAFETY: a signal to a process group id this process derived from its own
    // child, guarded above against the two wildcard values.
    unsafe {
        libc::kill(-pgid, signal);
    }
}

/// What a `k` frame does: ask politely, then insist.
///
/// A bare `SIGTERM` is not enough here. An interactive
/// login shell — which is what `POST /api/terminals` starts — *ignores*
/// `SIGTERM` by design, so a terminal the user closed in the browser would sit
/// there forever. The engine this replaces used `SIGKILL` outright
/// (`portable_pty`'s killer is `Child::kill`), so escalating is the behaviour
/// that was already there, with a chance to shut down cleanly added in front of
/// it.
fn terminate_shell(shell_pid: u32) {
    kill_shell(shell_pid, libc::SIGTERM);
    tokio::spawn(async move {
        tokio::time::sleep(KILL_ESCALATION).await;
        if crate::procs::alive(shell_pid) {
            tracing::info!(shell_pid, "the shell ignored SIGTERM; killing it");
            kill_shell(shell_pid, libc::SIGKILL);
        }
    });
}

/// Write the tombstone, the one record that outlives everything else.
fn write_tombstone(path: &Path, code: i32) {
    let tombstone = Tombstone {
        exit_code: code,
        at: crate::sessions::iso8601(std::time::SystemTime::now()),
    };
    match serde_json::to_vec(&tombstone) {
        Ok(encoded) => {
            if let Err(error) = std::fs::write(path, encoded) {
                tracing::error!(path = %path.display(), %error, "pty-shim: cannot write the exit file");
            }
        }
        Err(error) => tracing::error!(%error, "pty-shim: cannot encode the exit file"),
    }
}

/// A path's `(device, inode, ctime, ctime_ns)`, or `None` if it does not exist.
///
/// Device and inode alone are not enough: ext4 hands a freshly freed inode
/// number straight to the next file, so a successor bound at the same path
/// minutes later can wear the same `(dev, ino)` as the socket this shim bound
/// (APFS allocates monotonically, which is why this never showed on macOS).
/// The change time pins the identity to one particular creation.
///
/// Shared with [`crate::inject::shim`]: a session shim binds its socket, waits
/// out the same kind of linger and faces the same successor.
pub(crate) fn path_identity(path: &Path) -> Option<(u64, u64, i64, i64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path)
        .ok()
        .map(|meta| (meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec()))
}

/// Remove the socket only if the path still names the one this shim bound.
///
/// The naive `remove_file` is a real bug with a real window: this runs after a
/// 15-second linger, by which time the server may well have created a *new*
/// terminal that happened to reuse the id, or an operator may have replaced the
/// directory. Deleting a live successor's socket would strand a terminal that
/// nothing is wrong with. Comparing device and inode against what was bound
/// makes the removal unambiguous: same file, or nothing.
pub(crate) fn unlink_if_ours(path: &Path, identity: Option<(u64, u64, i64, i64)>) {
    let Some(identity) = identity else { return };
    if path_identity(path) != Some(identity) {
        tracing::info!(path = %path.display(), "the shim socket was replaced; leaving it alone");
        return;
    }
    let _ = std::fs::remove_file(path);
    // The `/tmp` fallback directory goes with the last socket in it. Recognised
    // by its prefix rather than by being told, so the shim can never be talked
    // into removing the instance directory: `remove_dir` also refuses anything
    // that is not empty.
    if let Some(parent) = path.parent() {
        if parent.to_string_lossy().starts_with("/tmp/remote-hand-") {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

/// Write keystrokes to the master fd, in order, off the runtime.
fn spawn_writer(mut writer: Box<dyn std::io::Write + Send>, input: std::sync::mpsc::Receiver<Vec<u8>>) {
    std::thread::spawn(move || {
        while let Ok(bytes) = input.recv() {
            if writer.write_all(&bytes).is_err() {
                break;
            }
            let _ = writer.flush();
        }
    });
}

/// The pty reader thread, the pipe that ends it, and how it says it ended.
struct Reader {
    wake: OwnedFd,
    join: std::thread::JoinHandle<()>,
    /// Exactly one [`ReaderEnd`], sent from the thread's own stack. Watched by
    /// [`keep_serving`], because this thread going away unnoticed is the whole
    /// incident.
    ended: mpsc::UnboundedReceiver<ReaderEnd>,
}

impl Reader {
    /// Wake the thread and wait for it. Blocking, so call it off the runtime.
    fn stop(self) {
        // SAFETY: `wake` is an owned, open descriptor and one byte is written
        // from a pointer valid for that byte.
        let _ = unsafe { libc::write(self.wake.as_raw_fd(), [0u8].as_ptr().cast(), 1) };
        let _ = self.join.join();
    }
}

/// Why a reader thread stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReaderEnd {
    /// The pty is finished with: EOF or `EIO` on the master, which means every
    /// handle on the slave is closed, or the wake pipe, which means
    /// [`Reader::stop`]. Both are ends somebody asked for.
    Closed,
    /// Anything else: a panic, or a `poll`/`read` that failed for a reason a
    /// dying shell does not produce. The thread is gone and the pty is not.
    Fault,
}

/// Reports a reader thread's ending as its stack goes away, returned or unwound.
///
/// A `Drop` rather than a value passed back from the thread body, because the
/// case this exists for is precisely the one where the body does not come back:
/// drops run during an unwind, so a panic is reported as [`ReaderEnd::Fault`]
/// instead of vanishing with the thread the way it did in production.
struct ReaderReport {
    end: ReaderEnd,
    to: mpsc::UnboundedSender<ReaderEnd>,
}

impl Drop for ReaderReport {
    fn drop(&mut self) {
        let _ = self.to.send(self.end);
    }
}

/// Start a reader thread running `body`, wired to a wake pipe and a report.
///
/// Split from [`spawn_reader`] so the supervision can be tested against a thread
/// that does exactly what a broken one does, without a pty and without a shell.
fn start_reader(body: impl FnOnce(&OwnedFd) -> ReaderEnd + Send + 'static) -> Result<Reader, String> {
    let mut ends = [0 as libc::c_int; 2];
    // SAFETY: `pipe` writes exactly the two descriptors this array holds.
    if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
        return Err(format!("pty-shim: cannot open a wake pipe: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: both descriptors are fresh and owned by nothing else.
    let (wake_rx, wake_tx) = unsafe { (OwnedFd::from_raw_fd(ends[0]), OwnedFd::from_raw_fd(ends[1])) };
    let (report, ended) = mpsc::unbounded_channel();

    let join = std::thread::spawn(move || {
        let mut report = ReaderReport {
            end: ReaderEnd::Fault,
            to: report,
        };
        report.end = body(&wake_rx);
    });
    Ok(Reader { wake: wake_tx, join, ended })
}

/// Read the master fd into the recorder until EOF or [`Reader::stop`].
///
/// `poll(2)` on a private `dup` rather than a plain blocking read, for the same
/// reason the in-process engine did it: a pty master only reports EOF once every
/// slave handle is closed, and a backgrounded grandchild holding the tty would
/// otherwise strand this thread for the life of the machine.
fn spawn_reader(master_fd: RawFd, shim: Arc<Shim>) -> Result<Reader, String> {
    // SAFETY: `master_fd` belongs to a master the shim owns for its whole life,
    // so it is open across the call; the dup is this thread's to close.
    let fd = unsafe { libc::dup(master_fd) };
    if fd < 0 {
        return Err(format!("pty-shim: cannot dup the master: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: `dup` returned a fresh descriptor nothing else owns.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };

    start_reader(move |wake| read_pty(&fd, wake, &shim))
}

/// The reader thread's body: poll, read, record, publish.
fn read_pty(fd: &OwnedFd, wake: &OwnedFd, shim: &Shim) -> ReaderEnd {
    let mut buffer = vec![0u8; READ_CHUNK];
    loop {
        let mut watched = [
            libc::pollfd {
                fd: fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: both descriptors are owned and open for the whole call, and
        // the array is exactly the length passed.
        let ready = unsafe { libc::poll(watched.as_mut_ptr(), 2, -1) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            // Not an end anybody asked for: a `poll` that fails on descriptors
            // this thread owns is this thread losing them, not a shell exiting.
            // Reported as a fault, so the supervisor starts a reader on a fresh
            // `dup` rather than leaving the pty with nobody on it.
            tracing::error!(%error, "pty-shim: the reader cannot poll the master");
            return ReaderEnd::Fault;
        }
        if watched[1].revents != 0 {
            return ReaderEnd::Closed;
        }
        if watched[0].revents == 0 {
            continue;
        }

        // SAFETY: `buffer` is owned and `read` writes at most its length.
        let read = unsafe { libc::read(fd.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
        if read < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            // `EIO` is how a pty master reports that the last slave closed —
            // the ordinary end of a shell. Anything else is this thread's own
            // descriptor going wrong, which a restart on a fresh one may fix.
            if error.raw_os_error() == Some(libc::EIO) {
                return ReaderEnd::Closed;
            }
            tracing::error!(%error, "pty-shim: the reader cannot read the master");
            return ReaderEnd::Fault;
        }
        if read == 0 {
            return ReaderEnd::Closed;
        }

        // The ring and the parser advance together, under one lock, so no
        // snapshot can ever describe a screen that never existed. A parse that
        // panics does not take this thread — and the lock — with it; what it
        // costs instead is the screen model, and that is said out loud.
        let (total, parsed) = {
            let mut recorder = shim.recorder();
            let parsed = recorder.append(&buffer[..read as usize]);
            (recorder.ring().total(), parsed)
        };
        if parsed == Parsed::Reset {
            tracing::error!("pty-shim: the screen model was rebuilt after a parse that panicked; the output is intact and the repaint is not");
            shim.faults.note("the screen model was rebuilt after a parse that panicked");
        }
        let _ = shim.total.send(total);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_needs_both_paths_and_a_separator() {
        let argv: Vec<String> = ["--socket", "/tmp/a.sock", "--exit-file", "/tmp/a.exit", "--", "/bin/sh", "-l"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let args = parse(&argv).expect("parse");
        assert_eq!(args.socket, PathBuf::from("/tmp/a.sock"));
        assert_eq!(args.exit_file, PathBuf::from("/tmp/a.exit"));
        assert_eq!(args.command, "/bin/sh");
        assert_eq!(args.args, vec!["-l".to_string()]);

        let missing_separator: Vec<String> = ["--socket", "/tmp/a.sock", "--exit-file", "/tmp/a.exit"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(parse(&missing_separator).is_err(), "the command must follow `--`");

        let empty_command: Vec<String> = ["--socket", "/s", "--exit-file", "/e", "--"].iter().map(|s| s.to_string()).collect();
        assert!(parse(&empty_command).is_err());

        let no_socket: Vec<String> = ["--exit-file", "/e", "--", "/bin/sh"].iter().map(|s| s.to_string()).collect();
        assert!(parse(&no_socket).is_err());

        // An argument that looks like a flag is only a flag *before* the
        // separator; after it, it belongs to the shell.
        let after: Vec<String> = ["--socket", "/s", "--exit-file", "/e", "--", "/bin/sh", "--socket", "-c", "echo"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(parse(&after).expect("parse").args, vec!["--socket", "-c", "echo"]);
    }

    #[test]
    fn the_socket_is_only_unlinked_when_the_path_still_names_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("terminal.sock");
        std::fs::write(&path, b"stand-in for a socket").expect("write");
        let identity = path_identity(&path).expect("identity");

        // A successor replaced the file: the shim must leave it alone. The
        // pause spans Linux's coarse file-time granularity (1-4 ms), the way
        // the 15 s linger does in production; without it ext4 can recreate the
        // file with the recycled inode *and* the same ctime tick.
        std::fs::remove_file(&path).expect("remove");
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, b"a successor's socket").expect("write");
        unlink_if_ours(&path, Some(identity));
        assert!(path.exists(), "a replaced socket belongs to whoever bound it");

        // The file this shim bound is removed.
        let identity = path_identity(&path).expect("identity");
        unlink_if_ours(&path, Some(identity));
        assert!(!path.exists());

        // And a shim that never bound anything removes nothing.
        std::fs::write(&path, b"someone else's").expect("write");
        unlink_if_ours(&path, None);
        assert!(path.exists());
    }

    /// A repaint of a large enough window does not fit in one frame, and a frame
    /// that does not fit is refused by the reader rather than the writer — so
    /// the attach fails, and fails again on every retry, for as long as the
    /// window is that size. The split is what makes it arrive at all; the
    /// character boundaries are what make it arrive intact.
    #[tokio::test]
    async fn a_repaint_too_large_for_one_frame_is_split_and_reassembles_byte_for_byte() {
        // Multi-byte characters all the way through, and of three different
        // widths, so a cut at any fixed offset lands inside one of them.
        let cell = "░ 漢 é ";
        let snapshot = cell.repeat(proto::MAX_PAYLOAD / cell.len() + 1024).into_bytes();
        assert!(snapshot.len() > proto::MAX_PAYLOAD, "the case is a repaint that does not fit");

        let frames = snapshot_frames(&snapshot, MAX_SNAPSHOT_FRAME);
        assert!(frames.len() > 1, "one frame is what the protocol cannot carry");
        let mut wire = Vec::new();
        for frame in &frames {
            wire.extend_from_slice(&frame.encode());
        }

        // Read it back the way the server does: whole frames off the wire, each
        // payload decoded on its own and appended. A frame over the limit fails
        // here, and a cut through a character shows up as a replacement one.
        let mut wire = wire.as_slice();
        let mut repainted = Vec::new();
        while let Some(frame) = proto::read_frame(&mut wire).await.expect("every frame is within the protocol's limit") {
            assert_eq!(frame.kind, proto::SNAPSHOT);
            repainted.extend_from_slice(String::from_utf8_lossy(&frame.payload).as_bytes());
        }
        assert_eq!(repainted, snapshot, "the repaint the client applies is the repaint the shim took");

        // The everyday repaint is unchanged: one frame, the same bytes in it.
        let small = b"\x1b[?1049l\x1b[H\x1b[Jhello".to_vec();
        let frames = snapshot_frames(&small, MAX_SNAPSHOT_FRAME);
        assert_eq!(frames, vec![Frame::raw(proto::SNAPSHOT, small)]);
        // And an empty one is still a frame, because the server is waiting for
        // one before it will show the terminal at all.
        assert_eq!(snapshot_frames(&[], MAX_SNAPSHOT_FRAME).len(), 1);
    }

    /// A loop that went down with nothing noticing — while the shim awaits an
    /// idle shell that will never exit — would leave a socket nobody accepts
    /// on. A loop that ends must be seen to end and be put back.
    #[tokio::test]
    async fn an_accept_loop_that_goes_down_is_restarted_rather_than_abandoned() {
        let (exited_tx, exited_rx) = tokio::sync::oneshot::channel::<i32>();
        let (restarted_tx, mut restarted_rx) = mpsc::unbounded_channel::<()>();
        let restarts = Arc::new(AtomicUsize::new(0));
        let counter = restarts.clone();

        // The first loop panics, which is the one way the real one can stop;
        // every loop after it stays up, the way a healthy one does.
        let mut accepting = tokio::spawn(async { panic!("deliberate: an accept loop going down") });
        let respawn = move || {
            counter.fetch_add(1, Ordering::SeqCst);
            let _ = restarted_tx.send(());
            tokio::spawn(std::future::pending::<()>())
        };
        // The shell exits only once the restart has happened, so this is the
        // order the bug had: a loop that dies long before the shell does.
        tokio::spawn(async move {
            restarted_rx.recv().await;
            let _ = exited_tx.send(7);
        });

        let post_mortem = PostMortem::new("t-restart");
        let mut reader = parked_reader();
        let ending = keep_serving(
            exited_rx,
            &mut accepting,
            respawn,
            &mut reader,
            no_reader_restart(),
            &post_mortem.faults,
            "t-restart",
        )
        .await;
        assert!(matches!(ending, Ending::Shell(7)), "the shell's own exit still ends the shim");
        assert_eq!(restarts.load(Ordering::SeqCst), 1, "the dead loop was noticed exactly once");
        assert!(!accepting.is_finished(), "the shim is left holding a loop that is accepting");
        assert!(
            post_mortem.read().contains("the accept loop ended"),
            "the one durable trace a shim with no stdio can leave: {:?}",
            post_mortem.read()
        );
    }

    /// And a loop that will not stay up is not a spin and not a shim that sits
    /// on an unserved socket: it is a tombstone and a shutdown, which is what
    /// makes the next boot sweep the terminal instead of adopting it.
    #[tokio::test]
    async fn an_accept_loop_that_will_not_stay_up_ends_the_shim() {
        // Held, not dropped: a dropped sender is a shell whose status was lost,
        // and this test is about the shell still running.
        let (_exited_tx, exited_rx) = tokio::sync::oneshot::channel::<i32>();
        let restarts = Arc::new(AtomicUsize::new(0));
        let counter = restarts.clone();

        let mut accepting = tokio::spawn(async {});
        let respawn = move || {
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async {})
        };

        let post_mortem = PostMortem::new("t-spin");
        let mut reader = parked_reader();
        let ending = keep_serving(
            exited_rx,
            &mut accepting,
            respawn,
            &mut reader,
            no_reader_restart(),
            &post_mortem.faults,
            "t-spin",
        )
        .await;
        assert!(matches!(ending, Ending::Deaf), "a shim that cannot accept does not stay up");
        assert_eq!(restarts.load(Ordering::SeqCst), ACCEPT_RESTARTS as usize, "the restarts are bounded");
    }

    /// A terminal's fault file and the directory it lives in, kept together so
    /// the directory outlives the notes written into it.
    struct PostMortem {
        _dir: tempfile::TempDir,
        path: PathBuf,
        faults: Faults,
    }

    impl PostMortem {
        fn new(id: &str) -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let exit_file = dir.path().join(format!("{id}.exit"));
            let faults = Faults::beside(&exit_file);
            Self {
                path: dir.path().join(format!("{id}.fault")),
                _dir: dir,
                faults,
            }
        }

        /// Everything noted so far, or an empty string if nothing was.
        fn read(&self) -> String {
            std::fs::read_to_string(&self.path).unwrap_or_default()
        }
    }

    /// A reader that stays put: it blocks until the wake pipe is written to or
    /// its writing end is dropped, which is what [`read_pty`]'s `poll` does.
    fn parked_reader() -> Reader {
        start_reader(|wake| {
            let mut byte = [0u8; 1];
            // SAFETY: `wake` is owned and open for the call, and one byte is
            // read into a buffer that holds one.
            let _ = unsafe { libc::read(wake.as_raw_fd(), byte.as_mut_ptr().cast(), 1) };
            ReaderEnd::Closed
        })
        .expect("start a reader")
    }

    /// The reader half of [`keep_serving`] for the tests that are not about it:
    /// a restart here would be a supervisor inventing a failure.
    fn no_reader_restart() -> impl FnMut() -> Result<Reader, String> {
        || panic!("the reader was healthy; nothing should have restarted it")
    }

    /// A reader thread that dies must not be left dead: around a shell that is
    /// perfectly alive, the recorder would never grow again, the browser would
    /// keep the snapshot from its last adoption, and every keystroke would go
    /// into a terminal nothing is reading.
    #[tokio::test]
    async fn a_reader_thread_that_dies_while_the_shell_lives_is_restarted() {
        let (exited_tx, exited_rx) = tokio::sync::oneshot::channel::<i32>();
        let (restarted_tx, mut restarted_rx) = mpsc::unbounded_channel::<()>();
        let restarts = Arc::new(AtomicUsize::new(0));
        let counter = restarts.clone();

        // A panic is how the real one vanished, and the report has to come out
        // of the unwind itself: the thread never reaches a line of its own
        // after this.
        let mut reader = start_reader(|_| panic!("deliberate: a reader that does not come back")).expect("start a reader");
        let respawn = move || -> Result<Reader, String> {
            counter.fetch_add(1, Ordering::SeqCst);
            let _ = restarted_tx.send(());
            Ok(parked_reader())
        };
        // The shell exits only after the restart, so this is the order the bug
        // had: a reader that dies long before the shell does.
        tokio::spawn(async move {
            restarted_rx.recv().await;
            let _ = exited_tx.send(7);
        });

        let post_mortem = PostMortem::new("t-reader");
        let mut accepting = tokio::spawn(std::future::pending::<()>());
        let ending = keep_serving(
            exited_rx,
            &mut accepting,
            || tokio::spawn(std::future::pending::<()>()),
            &mut reader,
            respawn,
            &post_mortem.faults,
            "t-reader",
        )
        .await;

        assert!(matches!(ending, Ending::Shell(7)), "the shell's own exit still ends the shim");
        assert_eq!(restarts.load(Ordering::SeqCst), 1, "the dead reader was noticed exactly once");
        let noted = post_mortem.read();
        assert!(
            noted.contains("the pty reader ended"),
            "and written down where a post-mortem can find it: {noted:?}"
        );
    }

    /// The end of the pty is not a fault. Restarting there would spend the
    /// budget on EOFs a fresh reader would meet just as fast, and turn an
    /// ordinary `exit` into a shim that reports `-1` and a terminal the next
    /// boot sweeps.
    #[tokio::test]
    async fn a_reader_that_reaches_the_end_of_the_pty_is_not_restarted() {
        let (exited_tx, exited_rx) = tokio::sync::oneshot::channel::<i32>();
        let mut reader = start_reader(|_| ReaderEnd::Closed).expect("start a reader");
        // The shell's status follows the EOF here, the usual order of a real
        // exit. It can also come first; that is `announce_exit`'s to handle.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = exited_tx.send(3);
        });

        let post_mortem = PostMortem::new("t-eof");
        let mut accepting = tokio::spawn(std::future::pending::<()>());
        let ending = keep_serving(
            exited_rx,
            &mut accepting,
            || tokio::spawn(std::future::pending::<()>()),
            &mut reader,
            no_reader_restart(),
            &post_mortem.faults,
            "t-eof",
        )
        .await;

        assert!(matches!(ending, Ending::Shell(3)), "the shell's status is what ends the shim");
        assert_eq!(post_mortem.read(), "", "an ordinary end is not a fault and leaves no note");
    }

    /// And a reader that will not stay up is not a spin and not a terminal
    /// frozen behind a socket that still answers: it is a tombstone and a
    /// shutdown, so the next boot sweeps the terminal instead of adopting it.
    #[tokio::test]
    async fn a_reader_that_will_not_stay_up_ends_the_shim() {
        // Held, not dropped: a dropped sender is a shell whose status was lost,
        // and this test is about the shell still running.
        let (_exited_tx, exited_rx) = tokio::sync::oneshot::channel::<i32>();
        let restarts = Arc::new(AtomicUsize::new(0));
        let counter = restarts.clone();

        let mut reader = start_reader(|_| ReaderEnd::Fault).expect("start a reader");
        let respawn = move || -> Result<Reader, String> {
            counter.fetch_add(1, Ordering::SeqCst);
            start_reader(|_| ReaderEnd::Fault)
        };

        let post_mortem = PostMortem::new("t-blind");
        let mut accepting = tokio::spawn(std::future::pending::<()>());
        let ending = keep_serving(
            exited_rx,
            &mut accepting,
            || tokio::spawn(std::future::pending::<()>()),
            &mut reader,
            respawn,
            &post_mortem.faults,
            "t-blind",
        )
        .await;

        assert!(matches!(ending, Ending::Blind), "a shim that cannot read its pty does not stay up");
        assert_eq!(restarts.load(Ordering::SeqCst), ACCEPT_RESTARTS as usize, "the restarts are bounded");
        let noted = post_mortem.read();
        assert!(noted.contains("the pty reader will not stay up"), "the shutdown says why: {noted:?}");
    }

    /// A shim with a pty and no shell under it: enough for the parts of
    /// [`Shim::mix`] that only need a recorder and one client.
    fn test_shim(dir: &Path) -> Arc<Shim> {
        let PtyPair { master, slave } = native_pty_system()
            .openpty(PtySize {
                rows: DEFAULT_ROWS,
                cols: DEFAULT_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        drop(slave);
        let (input, _) = std::sync::mpsc::channel::<Vec<u8>>();
        let (total, _) = watch::channel(0u64);
        Arc::new(Shim {
            recorder: Arc::new(Mutex::new(Recorder::new(DEFAULT_ROWS, DEFAULT_COLS))),
            master: Arc::new(Mutex::new(master)),
            input,
            shell_pid: 0,
            total,
            clients: Arc::new(Mutex::new(Vec::new())),
            live: Arc::new(AtomicUsize::new(0)),
            attached: Arc::new(AtomicBool::new(false)),
            exit_code: Arc::new(AtomicI64::new(NOT_EXITED)),
            faults: Faults::beside(&dir.join("t-hello.exit")),
        })
    }

    /// A panic that killed the reader would poison the recorder's lock; from
    /// then on the `HELLO` arm would take that lock one line before it sends
    /// its reply, so every client that dialled would have its task killed
    /// before a byte went out and none would get past the handshake. A
    /// poisoned lock must cost nothing but the panic that poisoned it.
    #[tokio::test]
    async fn a_poisoned_recorder_still_lets_a_client_finish_the_handshake() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = test_shim(dir.path());
        assert_eq!(shim.recorder().append(b"before the panic"), Parsed::Screen);

        // Poisoned the way a panicking reader does it: a panic with the lock
        // held, through the same `expect` that thread uses.
        let poisoning = shim.clone();
        let _ = std::thread::spawn(move || {
            let _held = poisoning.recorder.lock().expect("the lock is clean until this panics");
            panic!("deliberate: a panic with the recorder locked");
        })
        .join();
        assert!(shim.recorder.is_poisoned(), "the case is a poisoned lock");

        let (commands, command_rx) = mpsc::unbounded_channel::<Command>();
        let (frames, mut frame_rx) = mpsc::unbounded_channel::<Option<Frame>>();
        tokio::spawn(shim.clone().mix(command_rx, frames));
        commands
            .send(Command::Frame(Frame::json(proto::HELLO, &Hello { cursor: 0 })))
            .expect("the mixer is listening");

        let reply = frame_rx
            .recv()
            .await
            .expect("the mixer answers at all")
            .expect("a frame, not a half-close");
        assert_eq!(reply.kind, proto::HELLO, "the handshake completes");
        let hello2 = reply.parse::<Hello2>().expect("a hello the server can read");
        assert_eq!(hello2.total, 16, "and it carries the offsets the recorder really holds");

        // And the backlog follows it, which is what the browser sat without.
        let output = frame_rx.recv().await.expect("output follows the hello").expect("a frame");
        assert_eq!(output.kind, proto::OUTPUT);
        assert_eq!(output.payload, b"before the panic");
    }

    /// A client of `shim` that has said hello, and the frames it is sent.
    async fn attached_client(shim: &Arc<Shim>) -> mpsc::UnboundedReceiver<Option<Frame>> {
        let (commands, command_rx) = mpsc::unbounded_channel::<Command>();
        let (frames, mut frame_rx) = mpsc::unbounded_channel::<Option<Frame>>();
        // Registered the way `serve_client` registers it, so a broadcast reaches it.
        shim.clients.lock().expect("clients").push(commands.clone());
        tokio::spawn(shim.clone().mix(command_rx, frames));
        commands
            .send(Command::Frame(Frame::json(proto::HELLO, &Hello { cursor: 0 })))
            .expect("the mixer is listening");
        let hello = frame_rx.recv().await.expect("the mixer answers").expect("a frame");
        assert_eq!(hello.kind, proto::HELLO);
        frame_rx
    }

    /// Everything a client is sent until the shim half-closes it.
    async fn until_half_closed(frame_rx: &mut mpsc::UnboundedReceiver<Option<Frame>>) -> Vec<Frame> {
        let mut frames = Vec::new();
        while let Some(Some(frame)) = frame_rx.recv().await {
            frames.push(frame);
        }
        frames
    }

    /// `claude setup-token` prints the token and exits at once, and the shell's
    /// status can reach the shim before the reader has read the token off the
    /// master. Announced then, the exit frame goes out first, the mixer stops
    /// there, and the wizard is told the CLI exited without printing a token.
    /// The exit must wait for the reader.
    #[tokio::test]
    async fn the_exit_is_announced_only_after_the_shells_last_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = test_shim(dir.path());
        let mut frame_rx = attached_client(&shim).await;

        // A reader that is behind the shell: it records the last write only
        // when told to, the way a reader thread not yet scheduled would.
        let (release, released) = std::sync::mpsc::channel::<()>();
        let recording = shim.clone();
        let mut reader = start_reader(move |_| {
            let _ = released.recv();
            let total = {
                let mut recorder = recording.recorder();
                let _ = recorder.append(b"sk-ant-oat01-last-write");
                recorder.ring().total()
            };
            let _ = recording.total.send(total);
            ReaderEnd::Closed
        })
        .expect("start a reader");

        // The status is in and the reader is not. A grace far longer than this
        // test takes, so it is the reader that ends the wait and not the clock.
        let announcing = tokio::spawn({
            let shim = shim.clone();
            async move {
                shim.announce_exit(&mut reader, 0, Duration::from_secs(30)).await;
                reader
            }
        });
        let early = tokio::time::timeout(Duration::from_millis(100), frame_rx.recv()).await;
        assert!(early.is_err(), "nothing may reach the client while the reader is behind: {early:?}");

        release.send(()).expect("the reader is waiting");
        let reader = announcing.await.expect("the announcement completes");
        tokio::task::spawn_blocking(move || reader.stop()).await.expect("the reader stops");

        let frames = until_half_closed(&mut frame_rx).await;
        let kinds: Vec<u8> = frames.iter().map(|frame| frame.kind).collect();
        assert_eq!(kinds, vec![proto::OUTPUT, proto::EXIT], "the last output, then the exit: {frames:?}");
        assert_eq!(frames[0].payload, b"sk-ant-oat01-last-write");
        assert_eq!(frames[1].parse::<Exit>().expect("an exit frame").code, 0);
        assert_eq!(shim.exit_code.load(Ordering::SeqCst), 0, "and a late client is told the same");
    }

    /// A shell that exits as it starts, and a server that dials in the same
    /// instant: the exit is announced to the new client before its hello has
    /// been read. The client is owed its hello first (the server's handshake
    /// reads nothing else), then the backlog, then the exit. With the exit sent
    /// in the hello's place the handshake failed, the shim saw a client come
    /// and go and unlinked its socket, and the terminal was never created.
    #[tokio::test]
    async fn an_exit_announced_before_the_clients_hello_does_not_take_its_place() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = test_shim(dir.path());
        assert_eq!(shim.recorder().append(b"scanned\r\n"), Parsed::Screen);

        let (commands, command_rx) = mpsc::unbounded_channel::<Command>();
        let (frames, mut frame_rx) = mpsc::unbounded_channel::<Option<Frame>>();
        // Registered the way `serve_client` registers it, before its hello has
        // been read, so the announcement reaches it first.
        shim.clients.lock().expect("clients").push(commands.clone());
        let mut reader = parked_reader();
        shim.announce_exit(&mut reader, 7, Duration::from_millis(20)).await;
        tokio::task::spawn_blocking(move || reader.stop()).await.expect("the reader stops");

        tokio::spawn(shim.clone().mix(command_rx, frames));
        commands
            .send(Command::Frame(Frame::json(proto::HELLO, &Hello { cursor: 0 })))
            .expect("the mixer is listening");

        let frames = until_half_closed(&mut frame_rx).await;
        let kinds: Vec<u8> = frames.iter().map(|frame| frame.kind).collect();
        assert_eq!(kinds, vec![proto::HELLO, proto::OUTPUT, proto::EXIT], "the hello comes first: {frames:?}");
        assert_eq!(frames[1].payload, b"scanned\r\n");
        assert_eq!(frames[2].parse::<Exit>().expect("an exit frame").code, 7);
    }

    /// And the wait is bounded: a reader that never reaches EOF, because a
    /// backgrounded grandchild still holds the tty, does not keep the exit from
    /// being announced.
    #[tokio::test]
    async fn the_exit_is_announced_after_the_grace_when_the_pty_never_ends() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = test_shim(dir.path());
        let mut frame_rx = attached_client(&shim).await;
        let mut reader = parked_reader();

        shim.announce_exit(&mut reader, 3, Duration::from_millis(20)).await;
        let frames = until_half_closed(&mut frame_rx).await;
        tokio::task::spawn_blocking(move || reader.stop()).await.expect("the reader stops");

        assert_eq!(frames.len(), 1, "{frames:?}");
        assert_eq!(frames[0].kind, proto::EXIT);
        assert_eq!(frames[0].parse::<Exit>().expect("an exit frame").code, 3);
    }
    /// The shim command lines, pinned so the hand parser and clap answer alike.
    /// A value-taking flag consumes the next token (even `--`), repeats take the
    /// last, a bad number is ignored, and the separator is mandatory.
    #[test]
    fn the_pty_shim_argument_table() {
        let words = |line: &[&str]| -> Vec<String> { line.iter().map(|word| (*word).to_string()).collect() };
        let accepted: Vec<Vec<String>> = [
            &["--socket", "/s", "--exit-file", "/e", "--", "/bin/sh"][..],
            &["--exit-file", "/e", "--socket", "/s", "--", "/bin/sh", "-l"],
            &["--socket", "/s", "--socket", "/s2", "--exit-file", "/e", "--", "/bin/sh"],
            &["--socket", "--", "--exit-file", "/e", "--", "/bin/sh"],
            &["--socket", "/s", "--exit-file", "/e", "--cols", "80", "--rows", "24", "--", "/bin/sh"],
            &["--socket", "/s", "--exit-file", "/e", "--cols", "80", "--", "/bin/sh"],
            &["--socket", "/s", "--exit-file", "/e", "--cols", "x", "--rows", "y", "--", "/bin/sh"],
            &["--socket", "/s", "--exit-file", "/e", "--cols", "5", "--cols", "x", "--rows", "2", "--", "/bin/sh"],
            &["--socket", "/s", "--exit-file", "/e", "--", "/bin/sh", "--socket", "-c", "echo"],
        ]
        .into_iter()
        .map(words)
        .collect();
        for argv in &accepted {
            assert!(parse(argv).is_ok(), "{argv:?} must parse: {:?}", parse(argv).err());
        }
        let parsed = parse(&words(&["--socket", "/s", "--socket", "/s2", "--exit-file", "/e", "--", "/bin/sh"])).expect("parsed");
        assert_eq!(parsed.socket, PathBuf::from("/s2"), "the last --socket wins");
        let parsed = parse(&words(&["--socket", "--", "--exit-file", "/e", "--", "/bin/sh"])).expect("parsed");
        assert_eq!(parsed.socket, PathBuf::from("--"), "a `--` value");
        assert_eq!(parse(&words(&["--socket", "/s", "--exit-file", "/e", "--cols", "x", "--rows", "y", "--", "/bin/sh"])).expect("parsed").size, None);
        let sized = parse(&words(&["--socket", "/s", "--exit-file", "/e", "--cols", "80", "--rows", "24", "--", "/bin/sh"])).expect("parsed");
        assert_eq!(sized.size, Some((80, 24)));
        let half = parse(&words(&["--socket", "/s", "--exit-file", "/e", "--cols", "80", "--", "/bin/sh"])).expect("parsed");
        assert_eq!(half.size, None, "half a size is not a size");

        let refused: Vec<Vec<String>> = [
            &[][..],
            &["--socket", "/s", "--exit-file", "/e"],
            &["--socket", "/s", "--exit-file", "/e", "--"],
            &["--socket", "/s", "--exit-file", "/e", "/bin/sh"],
            &["--socket", "/s", "--exit-file", "/e", "--socket=/s2", "--", "/bin/sh"],
            &["--socket=/s", "--exit-file", "/e", "--", "/bin/sh"],
            &["--exit-file", "/e", "--", "/bin/sh"],
            &["--socket", "/s", "--", "/bin/sh"],
            &["--help"],
            &["--socket", "/s", "--exit-file", "/e", "--help", "--", "/bin/sh"],
            &["--socket", "--instance", "x", "--exit-file", "/e", "--", "/bin/sh"],
        ]
        .into_iter()
        .map(words)
        .collect();
        for argv in &refused {
            assert!(parse(argv).is_err(), "{argv:?} must be refused");
        }
    }

}
