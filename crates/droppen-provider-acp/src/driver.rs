//! One ACP session's driver: its queue, its turns, its transcript, and the
//! agent process behind them.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use agent_client_protocol_schema::v1::{RequestPermissionOutcome, SelectedPermissionOutcome};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Child;
use tokio::sync::{mpsc, oneshot};

use super::client::{Client, IncomingPermission};
use super::conn::{Conn, Event};
use crate::protocol::{category, option_for, ConfigOption, PermissionOptionKindWire, SessionUpdate, ToolKind};
use crate::agents::AcpAgent;
use droppen_provider::permissions::{self, PermissionSpec};
use droppen_provider::{InjectError, PermissionOutcome, PermissionReply, Phase, QueueOutcome, ResidentState, Sent};
use crate::sessions::{Record, Translator};
use droppen_sessions::delivery::{PermissionOption, PermissionOptionKind, PermissionState};
use droppen_sessions::{Injection, InjectionRunState, Source};
use droppen_base::time::iso8601;

/// How long an agent process with nothing to do is kept.
pub(super) const IDLE_EXIT: Duration = Duration::from_secs(30 * 60);
/// How much of the agent's stderr is kept for the message a failure is
/// reported with.
const STDERR_TAIL: usize = 4 * 1024;
/// How long a model or effort switch may take before the agent is taken for
/// dead: nothing else reaches it until that answer comes.
const CONFIG_TIMEOUT: Duration = if cfg!(test) { Duration::from_secs(5) } else { Duration::from_secs(60) };

/// The driver's state, readable without awaiting it.
#[derive(Clone, Debug, Default)]
pub(super) struct State {
    /// A `session/prompt` is in flight.
    active: bool,
    /// The driver has taken a message off the queue and is preparing its
    /// turn: setting the model and effort it asks for, which can take as long
    /// as `CONFIG_TIMEOUT`. The session reads as starting meanwhile.
    preparing: bool,
    /// How many turns this driver has started: the one in flight, when
    /// `active`, is the last of them. What a Stop names ([`Cmd::Interrupt`]).
    turns: u64,
    /// When it was sent.
    turn_started_at: Option<String>,
    /// Messages waiting for the turn in flight to end, in order.
    queue: VecDeque<Message>,
    /// The last turn this driver ran, as the session row reports it.
    injection: Option<Injection>,
    /// The session's configuration options as they stand: what its model and
    /// effort are, and may be set to.
    options: Vec<ConfigOption>,
    /// Oldest pending permission prompt's request id, if any.
    permission: Option<String>,
}

impl State {
    /// A driver's state as its session opens: the options the agent reports,
    /// and the last turn as its handle kept it on view ([`Handle::new`]).
    pub(super) fn opened(options: Vec<ConfigOption>, injection: Option<Injection>) -> Self {
        Self {
            options,
            injection,
            ..Self::default()
        }
    }
}

/// One message on its way to the agent.
#[derive(Clone, Debug)]
pub(super) struct Message {
    pub(super) id: String,
    /// What the user typed, as the transcript shows it.
    pub(super) text: String,
    /// What the agent is sent: the text and where its attachments are.
    pub(super) prompt: String,
    pub(super) images: Vec<String>,
    pub(super) files: Vec<String>,
    /// The model the turn is to run on, when the user picked one.
    pub(super) model: Option<String>,
    /// The effort it is to run at, when the user picked one.
    pub(super) effort: Option<String>,
}

/// What the send path and the routes hold on to.
#[derive(Clone)]
pub(crate) struct Handle {
    tx: mpsc::UnboundedSender<Cmd>,
    shared: Arc<Mutex<State>>,
    /// The id of the agent the session runs on.
    agent: Arc<str>,
    /// Messages sent to the driver that it has not taken yet.
    in_flight: Arc<AtomicUsize>,
    /// The agent's command line or environment changed: the process ends
    /// once it has nothing to do, and the next message starts a new one.
    retire: Arc<AtomicBool>,
}

/// A message on its way to the driver: what its sender waits on.
pub(super) struct Delivery {
    reply: oneshot::Receiver<Sent>,
}

/// One message in [`Handle::in_flight`], for as long as this lives: it goes
/// with the message, and the driver lets go of it once the message is in its
/// queue. A driver that is gone drops it with the message.
pub(crate) struct InFlight(Arc<AtomicUsize>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Delivery {
    /// Wait for the driver to take the message. Not under the spawn gate: a
    /// driver may take a while to read it, and nothing else should wait on it.
    pub(super) async fn taken(self, name: &str) -> Result<Sent, InjectError> {
        self.reply
            .await
            .map_err(|_| InjectError::message(format!("{name} exited before it took the message")))
    }
}

impl Handle {
    /// The handle of a process about to start, in place of `before`: how the
    /// last turn ended stays on view until this process runs the next one, as
    /// it was while no process ran.
    pub(super) fn new(tx: mpsc::UnboundedSender<Cmd>, agent: &str, before: Option<&Handle>) -> Self {
        Self {
            tx,
            shared: Arc::new(Mutex::new(State {
                injection: before.and_then(Handle::injection),
                ..State::default()
            })),
            agent: agent.into(),
            in_flight: Arc::default(),
            retire: Arc::default(),
        }
    }

    /// Whether this is a session of the agent `agent`.
    pub(super) fn runs_on(&self, agent: &str) -> bool {
        &*self.agent == agent
    }

    /// A turn is running, or a message is waiting for one — in the driver's
    /// queue, on its way there, or being prepared.
    pub(super) fn busy(&self) -> bool {
        let state = self.shared.lock().unwrap();
        self.alive() && (self.in_flight.load(Ordering::Acquire) > 0 || state.active || state.preparing || !state.queue.is_empty())
    }

    pub(super) fn shared(&self) -> Arc<Mutex<State>> {
        self.shared.clone()
    }

    /// Whether this handle is the one whose state is `shared`.
    pub(super) fn shares(&self, shared: &Arc<Mutex<State>>) -> bool {
        Arc::ptr_eq(&self.shared, shared)
    }

    /// Send the driver its next message, counted as in flight until the
    /// driver has it in its queue. A driver that is gone drops it, which
    /// [`Delivery::taken`] says.
    pub(super) fn deliver(&self, message: Message) -> Delivery {
        self.in_flight.fetch_add(1, Ordering::AcqRel);
        let counted = InFlight(self.in_flight.clone());
        let (reply, taken) = oneshot::channel();
        let _ = self.tx.send(Cmd::Deliver(message, reply, counted));
        Delivery { reply: taken }
    }

    /// Whether a message for this session goes to this process: it is alive
    /// and not on its way out. One retiring for a changed configuration runs
    /// only what it had when the configuration changed.
    pub(super) fn takes_messages(&self) -> bool {
        self.alive() && !self.retiring()
    }

    /// Ask the driver to end its process if it has nothing to do; the answer
    /// says whether it did. `None` when it is gone already.
    pub(super) fn request_stop(&self) -> Option<oneshot::Receiver<bool>> {
        let (reply, answer) = oneshot::channel();
        self.tx.send(Cmd::Stop(reply)).ok()?;
        Some(answer)
    }

    /// End the process once it has nothing to do ([`Handle::retire`]).
    pub(super) fn retire(&self) {
        self.retire.store(true, Ordering::Release);
    }

    pub(super) fn retiring(&self) -> bool {
        self.retire.load(Ordering::Acquire)
    }

    /// What the driver reads of its handle.
    pub(super) fn counters(&self) -> (Arc<AtomicUsize>, Arc<AtomicBool>) {
        (self.in_flight.clone(), self.retire.clone())
    }

    /// The session's configuration options as they stand.
    pub(super) fn options(&self) -> Vec<ConfigOption> {
        self.shared.lock().unwrap().options.clone()
    }

    pub(crate) fn alive(&self) -> bool {
        !self.tx.is_closed()
    }

    /// Whether the session row should report this driver at all.
    pub(crate) fn visible(&self) -> bool {
        self.alive()
    }

    pub(crate) fn injection(&self) -> Option<Injection> {
        self.shared.lock().unwrap().injection.clone()
    }

    /// No turn running, and the next one on its way: the driver is preparing
    /// it ([`State::preparing`]), or a message is on its way that no driver
    /// has taken, with none waiting, and the agent is starting for it (its
    /// process, then the session's opening), which can take as long as
    /// `START_TIMEOUT`. A driver that is running takes one at once.
    fn starting(&self, state: &State) -> bool {
        !state.active && (state.preparing || (state.queue.is_empty() && self.in_flight.load(Ordering::Acquire) > 0))
    }

    pub(crate) fn state(&self) -> ResidentState {
        let state = self.shared.lock().unwrap();
        ResidentState {
            phase: if state.active {
                Phase::Working
            } else if self.starting(&state) {
                Phase::Starting
            } else {
                Phase::Idle
            },
            stalled: false,
            queued: state.queue.len(),
            queued_messages: state
                .queue
                .iter()
                .map(|message| droppen_sessions::QueuedMessage {
                    id: message.id.clone(),
                    text: message.text.chars().take(200).collect(),
                    steered: false,
                })
                .collect(),
            background: 0,
            tasks: vec![],
            awaiting_background: false,
            turn_started_at: state.turn_started_at.clone(),
            // ACP sessions have no AskUserQuestion; interactive tool approvals
            // travel in `permission`.
            question: None,
            permission: state.permission.clone(),
            // The agent states each reading's window itself.
            model: None,
        }
    }

    pub(crate) fn info(&self) -> droppen_sessions::ResidentInfo {
        self.state().into()
    }

    /// Stop the turn running, or the one a starting agent is about to run:
    /// sent while the message is on its way, the interrupt waits behind it in
    /// the channel, and the driver cancels the turn as soon as it starts it.
    /// Either way it names that turn, so it stops no other: not the next one,
    /// if the turn it meant ends before the driver reads it.
    pub(crate) fn interrupt(&self) -> bool {
        let state = self.shared.lock().unwrap();
        let turn = if state.active {
            state.turns
        } else if self.starting(&state) {
            state.turns + 1
        } else {
            return false;
        };
        self.tx.send(Cmd::Interrupt(turn)).is_ok()
    }

    pub(super) async fn queue_action(&self, id: &str, steer: bool) -> QueueOutcome {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(Cmd::Queue { id: id.into(), steer, reply }).is_err() {
            return QueueOutcome::Unreachable;
        }
        rx.await.unwrap_or(QueueOutcome::Unreachable)
    }

    pub(super) async fn respond_permission(&self, request_id: &str, reply: PermissionReply, by: Option<String>) -> PermissionOutcome {
        let (respond, rx) = oneshot::channel();
        if self
            .tx
            .send(Cmd::RespondPermission {
                request_id: request_id.to_string(),
                reply,
                by,
                respond,
            })
            .is_err()
        {
            return PermissionOutcome::NoResident;
        }
        rx.await.unwrap_or(PermissionOutcome::NoResident)
    }
}

pub(crate) enum Cmd {
    /// A message, the reply its sender waits for, and its count in
    /// [`Handle::in_flight`].
    Deliver(Message, oneshot::Sender<Sent>, InFlight),
    /// Stop the driver's turn by this number ([`State::turns`]) if it is the
    /// one in flight; a turn that is over is not stopped, and neither is one
    /// after it.
    Interrupt(u64),
    Queue {
        id: String,
        steer: bool,
        reply: oneshot::Sender<QueueOutcome>,
    },
    /// Deliver the user's permission decision for a pending `session/request_permission`.
    RespondPermission {
        request_id: String,
        reply: PermissionReply,
        by: Option<String>,
        respond: oneshot::Sender<PermissionOutcome>,
    },
    /// End the process if nothing is in flight — no turn, no queue, no message
    /// on its way; `false` when something is.
    Stop(oneshot::Sender<bool>),
}

/// One `session/request_permission` currently awaiting the user's choice.
pub(super) struct OpenPermission {
    request_id: String,
    options: Vec<PermissionOption>,
    reply: oneshot::Sender<RequestPermissionOutcome>,
}

/// The agent process, leading a process group of its own: what it starts — a
/// helper, a language server, a shell it forked — is stopped with it.
pub(super) struct Process(Child);

impl Process {
    pub(super) fn new(child: Child) -> Self {
        Self(child)
    }

    /// Wait up to `grace` for it to exit on its own, then [`Process::kill`] it
    /// — exited or not, while it is unreaped, so what it left running in its
    /// group goes too.
    pub(super) async fn end(&mut self, grace: Duration) {
        let deadline = Instant::now() + grace;
        while !self.exited() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.kill().await;
    }

    /// Whether it has exited, seen without reaping it: `waitid` with
    /// `WNOWAIT` leaves it a zombie, so its pid still names its group.
    fn exited(&self) -> bool {
        let Some(pid) = self.0.id() else { return true };
        // SAFETY: a zeroed `siginfo_t` is a valid out-parameter, and `waitid`
        // with `WNOHANG | WNOWAIT` on a child of this process only reads its
        // state.
        unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            let found = libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT);
            // macOS reports a stopped child here too: only an exit counts.
            found == 0 && info.si_pid() != 0 && matches!(info.si_code, libc::CLD_EXITED | libc::CLD_KILLED | libc::CLD_DUMPED)
        }
    }

    /// Kill it and everything in its group, and reap it.
    pub(super) async fn kill(&mut self) {
        self.kill_group();
        let _ = tokio::time::timeout(KILL_WAIT, self.0.wait()).await;
    }

    /// `SIGKILL` its group. Only while it is unreaped: until then its pid, and
    /// so the group's id, cannot be anybody else's.
    fn kill_group(&self) {
        if let Some(pid) = self.0.id() {
            droppen_base::procs::signal_group(pid, libc::SIGKILL);
        }
    }
}

impl Drop for Process {
    /// A process dropped before it was reaped — by a check whose request was
    /// cancelled, or a session the server stopped with — takes its group with
    /// it.
    fn drop(&mut self) {
        self.kill_group();
    }
}

/// How long a killed agent is waited for.
const KILL_WAIT: Duration = Duration::from_secs(5);
/// How long the end of the agent's stderr is waited for once it is gone: a
/// failure is reported with what it said last, which can still be in the pipe.
const STDERR_SETTLE: Duration = Duration::from_secs(2);

/// The last [`STDERR_TAIL`] bytes the agent wrote to stderr.
#[derive(Clone)]
pub(super) struct StderrTail {
    kept: Arc<Mutex<Ring>>,
    /// `true` once stderr is closed.
    ended: tokio::sync::watch::Receiver<bool>,
}

/// The bytes [`StderrTail`] keeps.
#[derive(Debug, Default)]
struct Ring {
    bytes: VecDeque<u8>,
    /// Bytes were dropped off the front: the first line is the end of one.
    wrapped: bool,
}

impl Ring {
    fn push(&mut self, read: &[u8]) {
        self.bytes.extend(read);
        let excess = self.bytes.len().saturating_sub(STDERR_TAIL);
        if excess > 0 {
            self.bytes.drain(..excess);
            self.wrapped = true;
        }
    }

    /// The last few non-empty lines. A line the ring cut is left out: the
    /// start of a value it held is gone, so the rest of it could not be
    /// recognised to be redacted.
    fn text(&self) -> String {
        let bytes: Vec<u8> = self.bytes.iter().copied().collect();
        let text = String::from_utf8_lossy(&bytes);
        let whole = if self.wrapped {
            text.split_once('\n').map_or("", |(_, rest)| rest)
        } else {
            &text
        };
        let lines: Vec<&str> = whole.lines().filter(|line| !line.trim().is_empty()).collect();
        lines[lines.len().saturating_sub(8)..].join("\n")
    }
}

impl StderrTail {
    pub(super) fn follow(mut stderr: tokio::process::ChildStderr) -> Self {
        let (ended_tx, ended) = tokio::sync::watch::channel(false);
        let tail = Self {
            kept: Arc::default(),
            ended,
        };
        let kept = tail.kept.clone();
        tokio::spawn(async move {
            let mut buffer = [0u8; 4096];
            while let Ok(read) = stderr.read(&mut buffer).await {
                if read == 0 {
                    break;
                }
                kept.lock().unwrap().push(&buffer[..read]);
            }
            let _ = ended_tx.send(true);
        });
        tail
    }

    /// The last few non-empty lines.
    pub(super) fn text(&self) -> String {
        self.kept.lock().unwrap().text()
    }

    /// [`StderrTail::text`] once stderr is closed, or [`STDERR_SETTLE`] has
    /// passed: for a process that is gone, what it wrote last.
    pub(super) async fn settled(&self) -> String {
        let mut ended = self.ended.clone();
        let _ = tokio::time::timeout(STDERR_SETTLE, ended.wait_for(|ended| *ended)).await;
        self.text()
    }
}

/// The transcript file, opened for appending.
pub(super) struct Transcript {
    file: tokio::fs::File,
}

impl Transcript {
    pub(super) async fn open(path: &Path) -> std::io::Result<Self> {
        let file = tokio::fs::OpenOptions::new().create(true).append(true).open(path).await?;
        Ok(Self { file })
    }

    /// Append records. They are in the file when this returns: the feed is
    /// poked, or the session seen gone, right after, and whoever reads the
    /// transcript then must find them. A failed write is logged and the turn
    /// goes on: losing a line of the view is better than stopping the work it
    /// describes.
    pub(super) async fn append(&mut self, records: &[Value]) {
        if records.is_empty() {
            return;
        }
        let mut text = String::new();
        for record in records {
            text.push_str(&record.to_string());
            text.push('\n');
        }
        // A tokio file's write only hands the bytes to a blocking task; the
        // flush waits for that write to finish.
        let written = async {
            self.file.write_all(text.as_bytes()).await?;
            self.file.flush().await
        };
        if let Err(error) = written.await {
            tracing::warn!(%error, "cannot write an ACP transcript");
        }
    }
}

/// Why a message's model or effort was not set.
enum Unset {
    /// The agent has no such value, or refused it: the message fails, and the
    /// session goes on.
    Refused(String),
    /// The agent did not answer: the message fails, and the agent is taken for
    /// dead.
    Silent(String),
}

pub(super) struct Driver {
    pub(super) runs: Arc<super::AcpRuns>,
    /// The agent the session runs on, as it was configured when this process
    /// started.
    pub(super) agent: AcpAgent,
    /// This server's id for the session: what the feed, the receipts and the
    /// session row know it by.
    pub(super) id: String,
    /// The agent's id for it: what every request to the agent names, and what
    /// its notifications carry.
    pub(super) agent_session: String,
    pub(super) process: Process,
    pub(super) conn: Conn,
    pub(super) client: Arc<Client>,
    pub(super) stderr: StderrTail,
    pub(super) transcript: Transcript,
    pub(super) translator: Translator,
    pub(super) state: State,
    pub(super) shared: Arc<Mutex<State>>,
    pub(super) rx: mpsc::UnboundedReceiver<Cmd>,
    /// Incoming `session/request_permission` requests from the ACP client task.
    pub(super) perm_rx: mpsc::UnboundedReceiver<IncomingPermission>,
    /// Permission prompts this session is currently blocked on, oldest first.
    pub(super) pending_permissions: Vec<OpenPermission>,
    /// Its handle's count of messages on their way ([`Handle::deliver`]).
    pub(super) in_flight: Arc<AtomicUsize>,
    /// Its handle's [`Handle::retire`].
    pub(super) retire: Arc<AtomicBool>,
    /// A cancel was sent for the turn in flight.
    pub(super) interrupted: bool,
    /// The model and effort the transcript last recorded the session running
    /// on, so a `config` record is written only when they change.
    pub(super) recorded_choice: Option<(Option<String>, Option<String>)>,
}

impl Driver {
    fn publish(&mut self) {
        *self.shared.lock().unwrap() = self.state.clone();
        self.runs.host.poke_feed(Source::Acp, &self.id);
    }

    /// Nothing to do: no turn, no queue, no message on its way, and no pending permission.
    fn idle(&self) -> bool {
        !self.state.active
            && self.state.queue.is_empty()
            && self.in_flight.load(Ordering::Acquire) == 0
            && self.pending_permissions.is_empty()
    }

    /// Take this driver's handle out of the runs' map, if it is still
    /// the one there.
    fn leave(&self) {
        let key = self.runs.host.key_of(Source::Acp, &self.id);
        let mut servers = self.runs.acp_servers.lock().unwrap();
        if servers.get(&key).is_some_and(|handle| handle.shares(&self.shared)) {
            servers.remove(&key);
        }
    }

    pub(super) async fn run(mut self) {
        let mut idle_since = Instant::now();
        loop {
            if self.idle() && self.retire.load(Ordering::Acquire) {
                // The handle stays, closed, so the session row still says how
                // the last turn ended; the next message starts a new process.
                self.exit().await;
                return;
            }
            if !self.state.active {
                if let Some(message) = self.state.queue.pop_front() {
                    // Off the queue and not yet a turn: published as starting,
                    // so the session is not idle while the message's model or
                    // effort is set, and a Stop is for the turn it starts.
                    self.state.preparing = true;
                    self.publish();
                    if let Err(error) = self.submit(message).await {
                        self.died(&error.to_string()).await;
                        return;
                    }
                }
            }
            let idle_deadline = idle_since + IDLE_EXIT;
            tokio::select! {
                event = self.conn.events.recv() => match event {
                    Some(Event::Update(notification)) => {
                        if notification.session_id == self.agent_session {
                            if let SessionUpdate::ConfigOptionUpdate(update) = &notification.update {
                                self.options_changed(update.options());
                                self.publish();
                            }
                            let records = self.translator.update(&notification.update);
                            if !records.is_empty() {
                                self.transcript.append(&records).await;
                                self.runs.host.poke_feed(Source::Acp, &self.id);
                            }
                        }
                    }
                    Some(Event::Prompted(result)) => self.prompted(result).await,
                    Some(Event::Closed) | None => {
                        self.died(&format!("{} exited", self.agent.name)).await;
                        return;
                    }
                },
                Some(incoming) = self.perm_rx.recv() => {
                    self.open_permission(incoming);
                }
                cmd = self.rx.recv() => match cmd {
                    Some(Cmd::Stop(reply)) => {
                        let idle = self.idle();
                        if idle {
                            self.exit().await;
                        }
                        let _ = reply.send(idle);
                        if idle {
                            return;
                        }
                    }
                    Some(cmd) => {
                        if let Err(error) = self.command(cmd).await {
                            self.died(&error.to_string()).await;
                            return;
                        }
                    }
                    None => {
                        self.exit().await;
                        return;
                    }
                },
                _ = tokio::time::sleep_until(idle_deadline.into()), if self.idle() => {
                    // Gone from the map first, so the next message starts a new
                    // process instead of finding this one on its way out.
                    self.leave();
                    self.exit().await;
                    return;
                }
            }
            idle_since = Instant::now();
        }
    }

    async fn command(&mut self, cmd: Cmd) -> Result<(), InjectError> {
        match cmd {
            Cmd::Deliver(message, reply, counted) => {
                let queued = self.state.active || !self.state.queue.is_empty();
                self.state.queue.push_back(message);
                self.publish();
                // In the queue now, where the session's row and its busyness see it.
                drop(counted);
                let _ = reply.send(Sent { queued });
            }
            Cmd::Interrupt(turn) => {
                if self.state.active && self.state.turns == turn && !self.interrupted {
                    self.interrupted = true;
                    // ACP requires cancelling all pending permission requests when
                    // sending session/cancel so the agent unblocks promptly.
                    self.cancel_permissions();
                    self.conn.cancel(&self.agent_session).await?;
                }
            }
            Cmd::RespondPermission {
                request_id,
                reply,
                by,
                respond,
            } => {
                let outcome = self.respond_permission(&request_id, reply, by);
                let _ = respond.send(outcome);
            }
            Cmd::Queue { id, steer, reply } => {
                let Some(index) = self.state.queue.iter().position(|message| message.id == id) else {
                    let _ = reply.send(QueueOutcome::Unknown);
                    return Ok(());
                };
                if steer {
                    let _ = reply.send(QueueOutcome::Refused {
                        reason: format!("{} takes the next message when the turn in flight ends", self.agent.name),
                    });
                    return Ok(());
                }
                let dropped = self.state.queue.remove(index).expect("an index the queue just gave");
                self.publish();
                self.runs
                    .host.note_receipt(Source::Acp, &self.id, &dropped.id, droppen_sessions::delivery::ReceiptStatus::Failed);
                let _ = reply.send(QueueOutcome::Done);
            }
            Cmd::Stop(_) => unreachable!("handled by the loop"),
        }
        Ok(())
    }

    async fn submit(&mut self, message: Message) -> Result<(), InjectError> {
        self.transcript
            .append(&[Record::User {
                text: &message.text,
                images: &message.images,
                files: &message.files,
            }
            .stamped()])
            .await;
        if let Err(unset) = self.configure(&message).await {
            // The turn never starts: the session would have run on a model or
            // at an effort nobody asked for.
            self.runs
                .host.note_receipt(Source::Acp, &self.id, &message.id, droppen_sessions::delivery::ReceiptStatus::Failed);
            return match unset {
                Unset::Refused(reason) => {
                    let said = self.redact(&reason);
                    self.transcript.append(&[Record::Error { message: &said }.stamped()]).await;
                    self.start_turn();
                    self.end_turn(Some(said), false);
                    Ok(())
                }
                // Said once, by `died`, which this ends in: the turn it ends
                // with the end of the agent's stderr, and the queue behind it.
                Unset::Silent(reason) => {
                    self.start_turn();
                    Err(InjectError::message(reason))
                }
            };
        }
        self.record_choice().await;
        if let Err(error) = self.conn.prompt(&self.agent_session, message.prompt).await {
            // The prompt did not reach the agent, so the turn never started:
            // the message failed, and `died`, which this ends in, says so
            // once, as for a switch it never answered.
            self.runs
                .host
                .note_receipt(Source::Acp, &self.id, &message.id, droppen_sessions::delivery::ReceiptStatus::Failed);
            self.start_turn();
            return Err(error);
        }
        self.runs
            .host.note_receipt(Source::Acp, &self.id, &message.id, droppen_sessions::delivery::ReceiptStatus::Sent);
        self.start_turn();
        Ok(())
    }

    /// Mark a turn as started, on the row and in the mirror.
    fn start_turn(&mut self) {
        let now = iso8601(SystemTime::now());
        self.interrupted = false;
        self.state.active = true;
        self.state.preparing = false;
        self.state.turns += 1;
        self.state.turn_started_at = Some(now.clone());
        self.state.injection = Some(Injection {
            state: InjectionRunState::Running,
            error: droppen_http::openapi_ext::Nullable(None),
            interrupted: false,
            started_at: now,
            finished_at: droppen_http::openapi_ext::Nullable(None),
        });
        self.publish();
    }

    /// Set the session's model and effort to what the message picked, where
    /// they differ: one `session/set_config_option` each, answered within
    /// [`CONFIG_TIMEOUT`].
    async fn configure(&mut self, message: &Message) -> Result<(), Unset> {
        let name = self.agent.name.clone();
        let mut model_switched = false;
        if let Some(wanted) = message.model.as_deref() {
            let Some(option) = option_for(&self.state.options, category::MODEL).cloned() else {
                return Err(Unset::Refused(format!("{name} offers no model to choose")));
            };
            if option.current_value.as_deref() != Some(wanted) {
                if !option.accepts(wanted) {
                    return Err(Unset::Refused(format!("{name} has no model \"{wanted}\"")));
                }
                self.set_option(&name, "model", &option.id, wanted).await?;
                model_switched = true;
            }
        }
        if let Some(wanted) = message.effort.as_deref() {
            let Some(option) = option_for(&self.state.options, category::THOUGHT_LEVEL).cloned() else {
                // Switching to a model with no thinking levels (e.g. an
                // orchestrator or fixed-effort model) leaves effort unset
                // rather than failing the turn on a stale carried value.
                if model_switched {
                    return Ok(());
                }
                return Err(Unset::Refused(format!("{name} offers no effort to choose")));
            };
            let resolved = if option.accepts(wanted) {
                wanted.to_string()
            } else if model_switched {
                // Switching from a model with `xhigh` to one capped at `high`
                // clamps to the closest rung the new model offers (or keeps
                // its default) instead of aborting the turn.
                match resolve_effort_for_option(&option, wanted) {
                    Some(mapped) => mapped,
                    None => return Ok(()),
                }
            } else {
                return Err(Unset::Refused(format!("{name} has no effort \"{wanted}\"")));
            };
            if option.current_value.as_deref() != Some(resolved.as_str()) {
                self.set_option(&name, "effort", &option.id, &resolved).await?;
            }
        }
        // Published with the turn's start or its failure, not here: the session
        // reads as starting in between ([`State::preparing`]).
        Ok(())
    }

    async fn set_option(&mut self, name: &str, what: &str, option_id: &str, wanted: &str) -> Result<(), Unset> {
        let switched = tokio::time::timeout(CONFIG_TIMEOUT, self.conn.set_config_option(&self.agent_session, option_id, wanted)).await;
        let options = match switched {
            Ok(Ok(options)) => options,
            Ok(Err(error)) => return Err(Unset::Refused(format!("{name} did not switch its {what} to {wanted}: {error}"))),
            Err(_) => {
                return Err(Unset::Silent(format!(
                    "{name} did not answer a switch of its {what} to {wanted} within {}s; it was stopped",
                    CONFIG_TIMEOUT.as_secs()
                )))
            }
        };
        if options.is_empty() {
            // An agent that answers without the list: the value it took.
            if let Some(changed) = self.state.options.iter_mut().find(|candidate| candidate.id == option_id) {
                changed.current_value = Some(wanted.to_string());
            }
        } else {
            self.options_changed(options);
        }
        Ok(())
    }

    /// The session's options as the agent now reports them.
    fn options_changed(&mut self, options: Vec<ConfigOption>) {
        self.state.options = options;
    }

    /// Write down which model and effort the session runs on, when that is
    /// news: what the composer shows for a session it reopens. A value the
    /// model list would leave out ([`super::AcpRuns::acp_id_shown`]) is
    /// left out here too.
    async fn record_choice(&mut self) {
        let current = |category: &str| {
            option_for(&self.state.options, category)
                .and_then(|option| option.current_value.clone())
                .filter(|value| self.runs.acp_id_shown(value))
        };
        let choice = (current(category::MODEL), current(category::THOUGHT_LEVEL));
        if choice == (None, None) || self.recorded_choice.as_ref() == Some(&choice) {
            return;
        }
        self.transcript
            .append(&[Record::Config {
                model: choice.0.as_deref(),
                effort: choice.1.as_deref(),
            }
            .stamped()])
            .await;
        self.recorded_choice = Some(choice);
    }

    /// The turn in flight ended: say how, in the transcript and on the row.
    async fn prompted(&mut self, result: Result<crate::protocol::PromptResponse, String>) {
        if !self.state.active {
            return;
        }
        let mut records = self.translator.flush();
        let (error, cancelled) = match result {
            Ok(response) => {
                let stop = response.stop_reason.as_str();
                records.push(Record::TurnEnd { stop_reason: stop }.stamped());
                (None, stop == "cancelled")
            }
            Err(error) => {
                let message = self.redact(&error);
                records.push(Record::Error { message: &message }.stamped());
                (Some(message), false)
            }
        };
        self.transcript.append(&records).await;
        let interrupted = cancelled || (self.interrupted && error.is_some());
        self.end_turn(error, interrupted);
    }

    fn end_turn(&mut self, error: Option<String>, interrupted: bool) {
        self.cancel_permissions();
        self.interrupted = false;
        self.state.active = false;
        self.state.turn_started_at = None;
        let started_at = self
            .state
            .injection
            .as_ref()
            .map(|injection| injection.started_at.clone())
            .unwrap_or_else(|| iso8601(SystemTime::now()));
        let outcome = if error.is_some() || interrupted {
            InjectionRunState::Failed
        } else {
            InjectionRunState::Ok
        };
        self.state.injection = Some(Injection {
            state: outcome,
            error: droppen_http::openapi_ext::Nullable(error.clone()),
            interrupted,
            started_at,
            finished_at: droppen_http::openapi_ext::Nullable(Some(iso8601(SystemTime::now()))),
        });
        self.publish();
        if let Some(alerts) = self.runs.host.alerts.get() {
            alerts.turn(
                self.runs.host.paths.clone(),
                Source::Acp,
                self.id.clone(),
                outcome,
                error,
                self.runs.host.driver_states_of(&*self.runs),
            );
        }
    }

    /// What the agent said, fit to be stored and shown.
    fn redact(&self, text: &str) -> String {
        self.runs.redact_acp(&self.agent, text)
    }

    /// The process went away, or stopped answering: say so, and fail what
    /// was waiting for it.
    async fn died(&mut self, reason: &str) {
        self.cancel_permissions();
        self.client.kill_all();
        self.process.kill().await;
        let tail = self.stderr.settled().await;
        let message = self.redact(&if tail.is_empty() {
            reason.to_string()
        } else {
            format!("{reason}\n{tail}")
        });
        tracing::warn!(session = %self.id, agent = %self.agent.id, error = %message, "the agent process for this session is gone");
        let mut records = self.translator.flush();
        if self.state.active || !self.state.queue.is_empty() {
            records.push(Record::Error { message: &message }.stamped());
        }
        self.transcript.append(&records).await;
        for dropped in std::mem::take(&mut self.state.queue) {
            self.runs
                .host.note_receipt(Source::Acp, &self.id, &dropped.id, droppen_sessions::delivery::ReceiptStatus::Failed);
        }
        if self.state.active {
            let interrupted = self.interrupted;
            self.end_turn(Some(message), interrupted);
        } else {
            self.publish();
        }
        // The handle stays in the map with its sender closed, so the session
        // row still says how the last turn ended; the next message replaces it.
        self.rx.close();
    }

    /// End the process politely: an ACP agent over stdio exits when its input
    /// closes.
    async fn exit(&mut self) {
        self.cancel_permissions();
        self.rx.close();
        self.client.kill_all();
        self.conn.close().await;
        self.process.end(Duration::from_secs(10)).await;
    }

    fn open_permission(&mut self, incoming: IncomingPermission) {
        let request_id = droppen_provider::common::new_uuid();
        let tool = incoming.params.tool_call.as_ref();
        let tool_call_id = tool.map(|t| t.tool_call_id.clone()).filter(|id| !id.is_empty());
        let tool_name = tool.and_then(|t| t.kind).map(|kind| match kind {
            ToolKind::Execute => "shell",
            ToolKind::Read => "read",
            ToolKind::Edit => "edit",
            ToolKind::Delete => "delete",
            ToolKind::Move => "move",
            ToolKind::Search => "search",
            ToolKind::Fetch => "fetch",
            ToolKind::Think => "think",
            ToolKind::Other => "tool",
        });
        let raw_title = tool
            .and_then(|t| t.title.as_deref())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(tool_name.unwrap_or("Tool permission"));
        let title = permissions::clip_label(raw_title, |s| self.redact(s));
        let input = tool
            .and_then(|t| t.raw_input.as_ref())
            .and_then(|v| permissions::format_input_preview(v, |s| self.redact(s)));
        let options: Vec<PermissionOption> = incoming
            .params
            .options
            .into_iter()
            .filter(|opt| !opt.option_id.trim().is_empty())
            .map(|opt| {
                let kind = match opt.kind {
                    PermissionOptionKindWire::AllowOnce => PermissionOptionKind::AllowOnce,
                    PermissionOptionKindWire::AllowAlways => PermissionOptionKind::AllowAlways,
                    PermissionOptionKindWire::RejectOnce => PermissionOptionKind::RejectOnce,
                    PermissionOptionKindWire::RejectAlways => PermissionOptionKind::RejectAlways,
                    PermissionOptionKindWire::Other => PermissionOptionKind::Other,
                };
                let label = if opt.name.trim().is_empty() {
                    opt.option_id.clone()
                } else {
                    permissions::clip_label(&opt.name, |s| self.redact(s))
                };
                PermissionOption {
                    id: opt.option_id,
                    name: label,
                    kind,
                }
            })
            .collect();
        if options.is_empty() {
            let _ = incoming.reply.send(RequestPermissionOutcome::Cancelled);
            return;
        }
        let spec = PermissionSpec {
            request_id: request_id.clone(),
            session_id: self.id.clone(),
            tool_call_id,
            title: title.clone(),
            tool_name: tool_name.map(str::to_owned),
            reason: None,
            input,
            options: options.clone(),
        };
        let fresh = self.runs.host.permissions().open(&spec, permissions::now_ms());
        if fresh {
            if let Some(alerts) = self.runs.host.alerts.get() {
                alerts.question(
                    self.runs.host.paths.clone(),
                    Source::Acp,
                    self.id.clone(),
                    permissions::headline(&title),
                );
            }
        }
        self.pending_permissions.push(OpenPermission {
            request_id,
            options,
            reply: incoming.reply,
        });
        self.sync_permission_state();
        self.publish();
        self.runs.host.announce(Source::Acp, &self.id);
    }

    fn respond_permission(&mut self, request_id: &str, reply: PermissionReply, by: Option<String>) -> PermissionOutcome {
        let Some(at) = self.pending_permissions.iter().position(|p| p.request_id == request_id) else {
            return PermissionOutcome::Unknown;
        };
        let (to, selected, wire_outcome) = match reply {
            PermissionReply::Select(option_id) => {
                if !self.pending_permissions[at].options.iter().any(|opt| opt.id == option_id) {
                    return PermissionOutcome::Invalid(format!("unknown permission option: {option_id}"));
                }
                (
                    PermissionState::Resolved,
                    Some(option_id.clone()),
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id)),
                )
            }
            PermissionReply::Cancel => (PermissionState::Cancelled, None, RequestPermissionOutcome::Cancelled),
        };
        let store = self.runs.host.permissions();
        let now = permissions::now_ms();
        let stored = match store.resolve(request_id, to, selected.as_deref(), None, by.as_deref(), now) {
            Ok(stored) => stored,
            Err(Some(current)) => {
                let open = self.pending_permissions.remove(at);
                let _ = open.reply.send(RequestPermissionOutcome::Cancelled);
                self.sync_permission_state();
                self.publish();
                self.runs.host.announce(Source::Acp, &self.id);
                return PermissionOutcome::Resolved(current.wire());
            }
            Err(None) => return PermissionOutcome::Unknown,
        };
        let open = self.pending_permissions.remove(at);
        let _ = open.reply.send(wire_outcome);
        self.sync_permission_state();
        self.publish();
        self.runs.host.announce(Source::Acp, &self.id);
        PermissionOutcome::Delivered(stored.wire())
    }

    fn cancel_permissions(&mut self) {
        if self.pending_permissions.is_empty() {
            return;
        }
        let store = self.runs.host.permissions();
        let now = permissions::now_ms();
        for open in self.pending_permissions.drain(..) {
            store.close(&open.request_id, PermissionState::Cancelled, now);
            let _ = open.reply.send(RequestPermissionOutcome::Cancelled);
        }
        self.sync_permission_state();
        self.publish();
        self.runs.host.announce(Source::Acp, &self.id);
    }

    fn sync_permission_state(&mut self) {
        self.state.permission = self.pending_permissions.first().map(|p| p.request_id.clone());
    }
}

/// Relative intensity of common reasoning-effort names, used only to clamp a
/// carried effort when a model switch narrows the available `thought_level`
/// ladder (for example `xhigh` → `high`).
fn effort_rank(value: &str) -> Option<i32> {
    match value {
        "none" | "off" => Some(0),
        "minimal" | "min" => Some(1),
        "low" => Some(2),
        "medium" | "med" | "default" | "auto" => Some(3),
        "high" => Some(4),
        "xhigh" | "extra_high" | "extra-high" | "very_high" | "very-high" => Some(5),
        "max" | "maximum" => Some(6),
        _ => None,
    }
}

/// Nearest effort value `option` accepts for `wanted`, preferring not to
/// overshoot on ties; falls back to `option.current_value` when unranked.
fn resolve_effort_for_option(option: &ConfigOption, wanted: &str) -> Option<String> {
    if let Some(target) = effort_rank(wanted) {
        let best = option
            .values()
            .into_iter()
            .filter_map(|candidate| {
                let rank = effort_rank(&candidate.value)?;
                // Distance first; on ties prefer the lower/safer rung.
                Some(((rank - target).abs(), rank > target, candidate.value.as_str()))
            })
            .min_by_key(|(dist, higher, _)| (*dist, *higher))
            .map(|(_, _, value)| value.to_string());
        if best.is_some() {
            return best;
        }
    }
    option.current_value.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_clamps_to_nearest_supported_rung_after_model_switch() {
        let ladder = crate::protocol::config_options(&serde_json::json!({"configOptions": [
            {"id": "thought_level", "name": "Thinking Level", "category": "thought_level", "type": "select", "currentValue": "medium",
             "options": [{"value": "low", "name": "Low"}, {"value": "medium", "name": "Medium"}, {"value": "high", "name": "High"}]}
        ]}));
        let opt = &ladder[0];
        assert_eq!(resolve_effort_for_option(opt, "xhigh").as_deref(), Some("high"));
        assert_eq!(resolve_effort_for_option(opt, "max").as_deref(), Some("high"));
        assert_eq!(resolve_effort_for_option(opt, "minimal").as_deref(), Some("low"));
        assert_eq!(resolve_effort_for_option(opt, "custom_unknown").as_deref(), Some("medium"));
    }

    #[test]
    fn a_line_the_stderr_ring_cut_is_left_out() {
        let mut ring = Ring::default();
        ring.push(b"first\n\nsecond\n");
        assert_eq!(ring.text(), "first\nsecond", "nothing cut: every line");

        // 28 bytes, then enough to push all but the last 10 of them out.
        let mut ring = Ring::default();
        ring.push(b"token tok-very-secret-value\n");
        let mut rest = vec![b'x'; STDERR_TAIL - 21];
        rest.extend_from_slice(b"\nlast line\n");
        ring.push(&rest);
        let text = ring.text();
        assert!(!text.contains("value"), "the cut line's end is not shown: {text}");
        assert!(text.ends_with("xx\nlast line"), "{text}");
    }
}
