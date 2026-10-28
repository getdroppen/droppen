//! One ACP session's driver: its queue, its turns, its transcript, and the
//! agent process behind them.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Child;
use tokio::sync::{mpsc, oneshot};

use super::client::Client;
use super::conn::{Conn, Event};
use crate::acp::{category, option_for, ConfigOption, SessionUpdate};
use crate::acp_agent::AcpAgent;
use crate::inject::{InjectError, InjectionRegistry, Phase, QueueOutcome, ResidentState, Sent};
use crate::sessions::acp_transcript::{record, Translator};
use crate::sessions::{iso8601, Injection, InjectionRunState, Source};

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
    /// When it was sent.
    turn_started_at: Option<String>,
    /// Messages waiting for the turn in flight to end, in order.
    queue: VecDeque<Message>,
    /// The last turn this driver ran, as the session row reports it.
    injection: Option<Injection>,
    /// The session's configuration options as they stand: what its model and
    /// effort are, and may be set to.
    options: Vec<ConfigOption>,
}

impl State {
    pub(super) fn with_options(options: Vec<ConfigOption>) -> Self {
        Self { options, ..Self::default() }
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
pub(in crate::inject) struct Handle {
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
pub(in crate::inject) struct InFlight(Arc<AtomicUsize>);

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
    pub(super) fn new(tx: mpsc::UnboundedSender<Cmd>, agent: &str) -> Self {
        Self {
            tx,
            shared: Arc::default(),
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
    /// queue, or on its way there.
    pub(super) fn busy(&self) -> bool {
        let state = self.shared.lock().unwrap();
        self.alive() && (self.in_flight.load(Ordering::Acquire) > 0 || state.active || !state.queue.is_empty())
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

    pub(in crate::inject) fn alive(&self) -> bool {
        !self.tx.is_closed()
    }

    /// Whether the session row should report this driver at all.
    pub(in crate::inject) fn visible(&self) -> bool {
        self.alive()
    }

    pub(in crate::inject) fn injection(&self) -> Option<Injection> {
        self.shared.lock().unwrap().injection.clone()
    }

    pub(in crate::inject) fn state(&self) -> ResidentState {
        let state = self.shared.lock().unwrap();
        ResidentState {
            phase: if state.active { Phase::Working } else { Phase::Idle },
            stalled: false,
            queued: state.queue.len(),
            queued_messages: state
                .queue
                .iter()
                .map(|message| crate::sessions::QueuedMessage {
                    id: message.id.clone(),
                    text: message.text.chars().take(200).collect(),
                    steered: false,
                })
                .collect(),
            background: 0,
            tasks: vec![],
            awaiting_background: false,
            turn_started_at: state.turn_started_at.clone(),
            // Every permission request the agent makes is answered by the client.
            question: None,
        }
    }

    pub(in crate::inject) fn info(&self) -> crate::sessions::ResidentInfo {
        self.state().into()
    }

    pub(in crate::inject) fn interrupt(&self) -> bool {
        self.shared.lock().unwrap().active && self.tx.send(Cmd::Interrupt).is_ok()
    }

    pub(super) async fn queue_action(&self, id: &str, steer: bool) -> QueueOutcome {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(Cmd::Queue { id: id.into(), steer, reply }).is_err() {
            return QueueOutcome::Unreachable;
        }
        rx.await.unwrap_or(QueueOutcome::Unreachable)
    }
}

pub(in crate::inject) enum Cmd {
    /// A message, the reply its sender waits for, and its count in
    /// [`Handle::in_flight`].
    Deliver(Message, oneshot::Sender<Sent>, InFlight),
    Interrupt,
    Queue {
        id: String,
        steer: bool,
        reply: oneshot::Sender<QueueOutcome>,
    },
    /// End the process if nothing is in flight — no turn, no queue, no message
    /// on its way; `false` when something is.
    Stop(oneshot::Sender<bool>),
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
            crate::procs::signal_group(pid, libc::SIGKILL);
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

    /// Append records. A failed write is logged and the turn goes on: losing a
    /// line of the view is better than stopping the work it describes.
    pub(super) async fn append(&mut self, records: &[Value]) {
        if records.is_empty() {
            return;
        }
        let mut text = String::new();
        for record in records {
            text.push_str(&record.to_string());
            text.push('\n');
        }
        if let Err(error) = self.file.write_all(text.as_bytes()).await {
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
    pub(super) registry: Arc<InjectionRegistry>,
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
        self.registry.poke_feed(Source::Acp, &self.id);
    }

    /// Nothing to do: no turn, no queue, and no message on its way.
    fn idle(&self) -> bool {
        !self.state.active && self.state.queue.is_empty() && self.in_flight.load(Ordering::Acquire) == 0
    }

    /// Take this driver's handle out of the registry's map, if it is still
    /// the one there.
    fn leave(&self) {
        let key = self.registry.key_of(Source::Acp, &self.id);
        let mut servers = self.registry.acp_servers.lock().unwrap();
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
                                self.registry.poke_feed(Source::Acp, &self.id);
                            }
                        }
                    }
                    Some(Event::Prompted(result)) => self.prompted(result).await,
                    Some(Event::Closed) | None => {
                        self.died(&format!("{} exited", self.agent.name)).await;
                        return;
                    }
                },
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
            Cmd::Interrupt => {
                if self.state.active && !self.interrupted {
                    self.interrupted = true;
                    self.conn.cancel(&self.agent_session).await?;
                }
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
                self.registry
                    .note_receipt(Source::Acp, &self.id, &dropped.id, crate::inject::receipts::ReceiptStatus::Failed);
                let _ = reply.send(QueueOutcome::Done);
            }
            Cmd::Stop(_) => unreachable!("handled by the loop"),
        }
        Ok(())
    }

    async fn submit(&mut self, message: Message) -> Result<(), InjectError> {
        self.transcript
            .append(&[record(
                "user",
                json!({"text": message.text, "images": message.images, "files": message.files}),
            )])
            .await;
        if let Err(unset) = self.configure(&message).await {
            // The turn never starts: the session would have run on a model or
            // at an effort nobody asked for.
            self.registry
                .note_receipt(Source::Acp, &self.id, &message.id, crate::inject::receipts::ReceiptStatus::Failed);
            return match unset {
                Unset::Refused(reason) => {
                    let said = self.redact(&reason);
                    self.transcript.append(&[record("error", json!({ "message": said }))]).await;
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
        self.conn.prompt(&self.agent_session, message.prompt).await?;
        self.registry
            .note_receipt(Source::Acp, &self.id, &message.id, crate::inject::receipts::ReceiptStatus::Sent);
        self.start_turn();
        Ok(())
    }

    /// Mark a turn as started, on the row and in the mirror.
    fn start_turn(&mut self) {
        let now = iso8601(SystemTime::now());
        self.interrupted = false;
        self.state.active = true;
        self.state.turn_started_at = Some(now.clone());
        self.state.injection = Some(Injection {
            state: InjectionRunState::Running,
            error: crate::openapi_ext::Nullable(None),
            interrupted: false,
            started_at: now,
            finished_at: crate::openapi_ext::Nullable(None),
        });
        self.publish();
    }

    /// Set the session's model and effort to what the message picked, where
    /// they differ: one `session/set_config_option` each, answered within
    /// [`CONFIG_TIMEOUT`].
    async fn configure(&mut self, message: &Message) -> Result<(), Unset> {
        let name = self.agent.name.clone();
        let picks = [
            (category::MODEL, "model", message.model.as_deref()),
            (category::THOUGHT_LEVEL, "effort", message.effort.as_deref()),
        ];
        for (category, what, wanted) in picks {
            let Some(wanted) = wanted else { continue };
            let Some(option) = option_for(&self.state.options, category).cloned() else {
                return Err(Unset::Refused(format!("{name} offers no {what} to choose")));
            };
            if option.current_value.as_deref() == Some(wanted) {
                continue;
            }
            if !option.accepts(wanted) {
                return Err(Unset::Refused(format!("{name} has no {what} \"{wanted}\"")));
            }
            let switched = tokio::time::timeout(CONFIG_TIMEOUT, self.conn.set_config_option(&self.agent_session, &option.id, wanted)).await;
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
                if let Some(changed) = self.state.options.iter_mut().find(|candidate| candidate.id == option.id) {
                    changed.current_value = Some(wanted.to_string());
                }
            } else {
                self.options_changed(options);
            }
        }
        // Published with the turn's start or its failure, not here: the message
        // is off the queue and the turn not started, and the session must not
        // look idle in between — the agent could be removed under it.
        Ok(())
    }

    /// The session's options as the agent now reports them.
    fn options_changed(&mut self, options: Vec<ConfigOption>) {
        self.state.options = options;
    }

    /// Write down which model and effort the session runs on, when that is
    /// news: what the composer shows for a session it reopens. A value the
    /// model list would leave out ([`InjectionRegistry::acp_id_shown`]) is
    /// left out here too.
    async fn record_choice(&mut self) {
        let current = |category: &str| {
            option_for(&self.state.options, category)
                .and_then(|option| option.current_value.clone())
                .filter(|value| self.registry.acp_id_shown(value))
        };
        let choice = (current(category::MODEL), current(category::THOUGHT_LEVEL));
        if choice == (None, None) || self.recorded_choice.as_ref() == Some(&choice) {
            return;
        }
        self.transcript
            .append(&[record("config", json!({"model": choice.0, "effort": choice.1}))])
            .await;
        self.recorded_choice = Some(choice);
    }

    /// The turn in flight ended: say how, in the transcript and on the row.
    async fn prompted(&mut self, result: Result<crate::acp::PromptResponse, String>) {
        if !self.state.active {
            return;
        }
        let mut records = self.translator.flush();
        let (error, cancelled) = match result {
            Ok(response) => {
                let stop = response.stop_reason.as_str();
                records.push(record("turn_end", json!({ "stopReason": stop })));
                (None, stop == "cancelled")
            }
            Err(error) => {
                let message = self.redact(&error);
                records.push(record("error", json!({ "message": message })));
                (Some(message), false)
            }
        };
        self.transcript.append(&records).await;
        let interrupted = cancelled || (self.interrupted && error.is_some());
        self.end_turn(error, interrupted);
    }

    fn end_turn(&mut self, error: Option<String>, interrupted: bool) {
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
            error: crate::openapi_ext::Nullable(error.clone()),
            interrupted,
            started_at,
            finished_at: crate::openapi_ext::Nullable(Some(iso8601(SystemTime::now()))),
        });
        self.publish();
        crate::push::notify_turn(
            self.registry.paths.clone(),
            Source::Acp,
            self.id.clone(),
            outcome,
            error,
            self.registry.driver_states(),
        );
    }

    /// What the agent said, fit to be stored and shown.
    fn redact(&self, text: &str) -> String {
        self.registry.redact_acp(&self.agent, text)
    }

    /// The process went away, or stopped answering: say so, and fail what
    /// was waiting for it.
    async fn died(&mut self, reason: &str) {
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
            records.push(record("error", json!({ "message": message })));
        }
        self.transcript.append(&records).await;
        for dropped in std::mem::take(&mut self.state.queue) {
            self.registry
                .note_receipt(Source::Acp, &self.id, &dropped.id, crate::inject::receipts::ReceiptStatus::Failed);
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
        self.rx.close();
        self.client.kill_all();
        self.conn.close().await;
        self.process.end(Duration::from_secs(10)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
