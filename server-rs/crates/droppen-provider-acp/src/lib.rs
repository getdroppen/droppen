//! Agents driven over the Agent Client Protocol: one agent process per
//! session, spoken to over stdio.
//!
//! [ACP](https://agentclientprotocol.com) is JSON-RPC 2.0, one message per
//! line. This server is its client half ([`conn`] for the wire, [`crate::protocol`]
//! for the messages), and uses the part of the protocol that holds a
//! conversation:
//!
//! * `initialize`, advertising the file and terminal methods: an agent may run
//!   its shell and file tools through the client, and [`client`] is that
//!   client;
//! * `session/new` for a new session, `session/load` to reopen one — which
//!   replays the session's history as `session/update` notifications;
//! * `session/prompt` per message, answered with a stop reason when the turn
//!   ends, with the turn itself streamed as `session/update` notifications in
//!   between;
//! * `session/set_config_option` before a turn whose message picked another
//!   model or effort than the session has: the agent publishes both as
//!   configuration options (categories `model` and `thought_level`), and
//!   [`AcpRuns::acp_options`] reads them for the composer before any
//!   session exists, by opening one in a scratch directory and closing it;
//! * `session/cancel` to stop a turn.
//!
//! What the agent says is written down as it arrives, in the transcript
//! [`crate::sessions`] keeps; that file is what the session
//! view reads.
//!
//! Everything here is the same for every ACP agent. Which agents there are is
//! the user's to configure ([`crate::agents`]); each session records the one
//! it runs on, and is started, reopened and resumed with that agent and no
//! other. Its id is this server's own and the agent's id is kept beside it
//! ([`crate::sessions`]): two agents may give two sessions the
//! same id, and both still have to be told apart here.
//!
//! One message runs at a time. A message sent mid-turn waits in the driver's
//! queue and goes in when the turn ends: ACP takes a second `session/prompt`
//! only behind the first, so holding it here is the same thing with a queue
//! the user can see and take a message back out of.
//!
//! Unlike the Claude and Codex residents, an ACP agent is not started through
//! the session shim and is not journalled: it is this server's child, and it
//! ends with this server. A turn in flight when the server stops is cut short
//! (what it said so far is in the transcript), and the next message reopens the
//! session with `session/load`. A process left alone for [`driver::IDLE_EXIT`]
//! is closed, for the same reason: the agent keeps the conversation, and an idle
//! process per session is not worth keeping for it.

pub mod agents;
mod client;
mod conn;
mod driver;
pub mod models;
pub mod protocol;
pub mod sessions;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use droppen_provider::common::{new_uuid, session_workdir, QueueOutcome, ResidentState};
use droppen_provider::{AccountChoice, Capabilities, DriverStatesBuilder, InjectError, InjectionRegistry, Provider, ProviderRuns, Run, RunHost, SendOptions, SendTo, Sent, SpawnGate, SpawnPermit, StartAt, WrittenAt};
use futures_util::future::BoxFuture;
use crate::protocol::{category, option_for, ConfigOption, InitializeResponse};
use crate::agents::{AcpAgent, AcpAgentRegistry, AcpCheck, AcpCheckState};
use crate::sessions::{has_conversation, read_header, AcpTranscripts, Record, Translator};
use droppen_sessions::{Injection, SessionSummary, Source};
use droppen_http::error::ApiError;
use droppen_base::process_env::clean_env_bare;
use client::Client;
use conn::Conn;
pub(crate) use driver::Handle;
use driver::{Cmd, Delivery, Driver, Message, Process, State, StderrTail};
use serde_json::Value;
use tokio::sync::mpsc;

/// How long an agent may take to start and open a session.
///
/// Generous: `initialize` may import a whole framework and its plugins (an
/// agent may be a Python program), and `session/load` reads and replays the
/// whole history.
const START_TIMEOUT: Duration = Duration::from_secs(120);

/// What to do when an agent cannot be started, said after the reason.
const INSTALL_HINT: &str = "Check its executable and arguments in Settings";

/// How long a check waits for `initialize` to be answered.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// A session this server already has a transcript for.
struct Recorded {
    /// Its transcript.
    file: PathBuf,
    /// The id the agent gave it.
    agent_session: String,
    /// Where it ran.
    cwd: Option<String>,
}

/// The prompt an agent is sent: the text, and where each attachment is on disk.
///
/// Paths rather than inline content, images included: an ACP agent reads
/// files with its own tools, and an image block would fail outright on a
/// model without vision.
fn prompt_text(text: &str, images: &[String], files: &[String]) -> String {
    let attached: Vec<&String> = images.iter().chain(files).collect();
    if attached.is_empty() {
        return text.to_string();
    }
    let list = attached.iter().map(|path| format!("- {path}")).collect::<Vec<_>>().join("\n");
    format!("{text}\n\n[{} attached file(s):]\n{list}", attached.len())
}

fn message(text: &str, options: &SendOptions) -> Message {
    let picked = |value: &Option<String>| value.as_deref().map(str::trim).filter(|value| !value.is_empty()).map(str::to_owned);
    Message {
        id: options.message_id.clone().unwrap_or_else(new_uuid),
        text: text.to_string(),
        prompt: prompt_text(text, &options.images, &options.files),
        images: options.images.clone(),
        files: options.files.clone(),
        model: picked(&options.model),
        effort: picked(&options.effort),
    }
}

/// Refuse what an ACP session cannot do differently per message.
///
/// An ACP agent has no accounts here and no permission modes; a request that
/// names either is refused rather than quietly run on something else. Its
/// model and effort are its own configuration options, checked against what
/// it publishes ([`check_choice`]).
fn check_options(agent: &AcpAgent, options: &SendOptions) -> Result<(), InjectError> {
    let name = &agent.name;
    if options.account.named().is_some() || matches!(options.account, AccountChoice::Rule(_)) {
        return Err(InjectError::message(format!(
            "{name} runs on its own configuration; there is no account to choose"
        )));
    }
    let named = |value: &Option<String>| value.as_deref().is_some_and(|value| !value.trim().is_empty());
    if named(&options.permission) {
        return Err(InjectError::message(format!("{name} has no permission modes to choose between")));
    }
    if options.chat {
        return Err(InjectError::message(format!(
            "instant chats run on Claude Code, Codex or an API provider, not {name}"
        )));
    }
    Ok(())
}

/// Refuse a model or effort the agent does not offer, when what it offers is
/// known; with nothing known yet, the agent itself answers when it is set.
fn check_choice(agent: &AcpAgent, offered: &[ConfigOption], model: Option<&str>, effort: Option<&str>) -> Result<(), InjectError> {
    if offered.is_empty() {
        return Ok(());
    }
    for (category, what, wanted) in [(category::MODEL, "model", model), (category::THOUGHT_LEVEL, "effort", effort)] {
        let Some(wanted) = wanted else { continue };
        match option_for(offered, category) {
            None => return Err(InjectError::message(format!("{} offers no {what} to choose", agent.name))),
            Some(option) if !option.accepts(wanted) => {
                return Err(InjectError::message(format!("{} has no {what} \"{wanted}\"", agent.name)));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// An agent process, started and connected, before any session is open.
struct Started {
    process: Process,
    stderr: StderrTail,
    client: Arc<Client>,
    conn: Conn,
}

/// Start `agent` in `workdir` for a session, and connect to it: only under a
/// permit, which a start takes under the spawn gate.
async fn start(agent: &AcpAgent, workdir: &Path, _permit: &SpawnPermit<'_>) -> Result<Started, InjectError> {
    launch(agent, workdir).await
}

/// Start `agent` in a scratch directory, to ask it something: no session, so
/// no permit and no gate.
async fn start_scratch(agent: &AcpAgent, scratch: &ScratchDir) -> Result<Started, InjectError> {
    launch(agent, scratch.path()).await
}

/// Start `agent` in `workdir` and connect to it: its command line, over stdio,
/// with its variables laid over the server's clean environment, in a process
/// group of its own ([`Process`]).
async fn launch(agent: &AcpAgent, workdir: &Path) -> Result<Started, InjectError> {
    let program = agent.program();
    if program.as_os_str().is_empty() {
        return Err(InjectError::message(format!("{} has no executable. {INSTALL_HINT}", agent.name)));
    }
    let env = agent.environment(clean_env_bare());
    let mut child = tokio::process::Command::new(&program)
        .args(&agent.args)
        .current_dir(workdir)
        .env_clear()
        .envs(&env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            InjectError::message(format!(
                "cannot start {} ({}): {error}. {INSTALL_HINT}",
                agent.name,
                program.display(),
            ))
        })?;
    let stderr = StderrTail::follow(child.stderr.take().expect("piped stderr"));
    let client = Client::new(workdir.to_path_buf(), env);
    let conn = Conn::start(
        child.stdin.take().expect("piped stdin"),
        child.stdout.take().expect("piped stdout"),
        client.clone(),
    )
    .await?;
    Ok(Started {
        process: Process::new(child),
        stderr,
        client,
        conn,
    })
}

impl Started {
    /// End a process that did not get as far as a driver, and say why.
    async fn fail(mut self, runs: &AcpRuns, agent: &AcpAgent, reason: String) -> InjectError {
        self.client.kill_all();
        self.process.kill().await;
        let tail = self.stderr.settled().await;
        let said = if tail.is_empty() { reason } else { format!("{reason}\n{tail}") };
        InjectError::message(runs.redact_acp(agent, &format!("cannot start {}: {said}", agent.name)))
    }
}

/// A session whose agent is being started, outside the spawn gate. Its handle
/// is in the map already, with the first message on its way, so the session
/// is busy — it is not removed, archived or started twice — and a message
/// sent to it meanwhile queues behind the first.
///
/// Dropped before [`AcpRuns::acp_attach`] took it — the start
/// failed, or its request was given up on — it takes its handle out again and
/// puts back the one it replaced, and says so to the session's feed and the
/// list; the messages on their way are dropped, and their senders told the
/// agent exited before it took them.
struct Starting<'a> {
    runs: &'a AcpRuns,
    /// The session's id.
    id: String,
    handle: Handle,
    rx: Option<mpsc::UnboundedReceiver<Cmd>>,
    replaced: Option<Handle>,
}

impl Drop for Starting<'_> {
    fn drop(&mut self) {
        if self.rx.is_none() {
            return;
        }
        let key = self.runs.host.key_of(Source::Acp, &self.id);
        let mut servers = self.runs.acp_servers.lock().unwrap();
        if servers.get(&key).is_some_and(|handle| handle.shares(&self.handle.shared())) {
            match self.replaced.take() {
                Some(replaced) => servers.insert(key, replaced),
                None => servers.remove(&key),
            };
            drop(servers);
            self.runs.host.announce(Source::Acp, &self.id);
        }
    }
}

/// A session's agent, started, with the session open and its transcript
/// ready: what a driver is made of.
struct Open {
    agent_session: String,
    started: Started,
    transcript: driver::Transcript,
    options: Vec<ConfigOption>,
}

/// A scratch directory for an agent that is only asked something: removed
/// when it is dropped, a cancelled request's included.
struct ScratchDir(tempfile::TempDir);

impl ScratchDir {
    fn path(&self) -> &Path {
        self.0.path()
    }
}

/// A new [`ScratchDir`], the only one an agent is started in without a permit.
fn scratch(kind: &str) -> std::io::Result<ScratchDir> {
    tempfile::Builder::new().prefix(&format!("droppen-acp-{kind}-")).tempdir().map(ScratchDir)
}

/// Every configured ACP agent, as the provider registry knows them.
pub struct Acp {
    /// Its agents' processes and what they offer.
    runs: Arc<AcpRuns>,
    /// Each agent's models.
    models: crate::models::AcpModels,
}

impl Acp {
    /// The ACP agents, run on `registry`'s host.
    pub fn new(registry: Arc<InjectionRegistry>) -> Self {
        let runs = AcpRuns::new(registry.host.clone());
        let models = crate::models::AcpModels::new(runs.host.paths.clone(), runs.clone());
        Self { runs, models }
    }

    /// Its runs, for the ACP agents' own routes.
    pub(crate) fn runs(&self) -> &Arc<AcpRuns> {
        &self.runs
    }
}

/// The ACP agents' runs: the agent process behind each session, what each
/// agent offers, and the probes that ask it. Kept here rather than on the
/// host they run on, whose shared parts — the paths, the runs journal, the
/// feeds — they reach through it.
pub(crate) struct AcpRuns {
    /// What they run on.
    host: Arc<RunHost>,
    /// The agent process behind each ACP session, keyed
    /// [`droppen_provider::RunHost::key_of`].
    acp_servers: Mutex<HashMap<String, Handle>>,
    /// What each ACP agent last said it offers, by agent id: its model list
    /// and effort ladder, for the composer; see [`AcpRuns::acp_options`].
    acp_options: Mutex<HashMap<String, Vec<ConfigOption>>>,
    /// Serializes each agent's probes, so a page asking twice starts one
    /// process, not two.
    acp_probes: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl AcpRuns {
    /// None yet, on `host`.
    fn new(host: Arc<RunHost>) -> Arc<Self> {
        let runs = Arc::new(Self {
            host,
            acp_servers: Mutex::new(HashMap::new()),
            acp_options: Mutex::new(HashMap::new()),
            acp_probes: Mutex::new(HashMap::new()),
        });
        runs
    }
}

/// An ACP agent's process is never journalled, so no run of one is adopted:
/// it dies with the server that started it.
impl ProviderRuns for AcpRuns {
    fn source(&self) -> Source {
        Source::Acp
    }

    fn host(&self) -> &Arc<RunHost> {
        &self.host
    }

    fn add_to(&self, states: &mut DriverStatesBuilder) {
        self.acp_driver_states(states);
    }

    fn cli_name(&self) -> &'static str {
        "the ACP agent"
    }

    fn readable(&self, _record: &droppen_provider::runs::RunRecord) -> bool {
        true
    }

    // No ACP run names a shim: an agent is not started behind one.
    fn attach(self: Arc<Self>, record: droppen_provider::runs::RunRecord, _sidecars: droppen_provider::shim::Sidecars, _link: droppen_provider::shim::Link, _lease: droppen_provider::gate::SessionLease) {
        tracing::warn!(session = %record.session_id, "an ACP run names a shim; it is let go");
    }

    fn was_cut_short<'a>(&'a self, _record: &'a droppen_provider::runs::RunRecord) -> BoxFuture<'a, bool> {
        Box::pin(std::future::ready(false))
    }

    fn last_write<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Option<std::time::Instant>> {
        Box::pin(async move { AcpTranscripts::of(&self.host.paths).locate(id).await?.1.written_at().await })
    }
}

impl Provider for Acp {
    fn source(&self) -> Source {
        Source::Acp
    }

    /// Every permission request an agent makes is answered by the client:
    /// an agent never stops to ask the user. Its sessions are conversations
    /// only: no instant chats, and no scheduled tasks.
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    fn run(&self) -> &dyn Run {
        self
    }

    fn runs(&self) -> Arc<dyn ProviderRuns> {
        self.runs.clone()
    }

    // An agent runs on its own configuration and takes no account.
    fn continue_on_account<'a>(&'a self, _id: &'a str, _new_id: &'a str) -> BoxFuture<'a, Result<std::path::PathBuf, droppen_provider::ContinueError>> {
        Box::pin(std::future::ready(Err(droppen_provider::ContinueError::Unsupported)))
    }

    fn models(&self) -> &dyn droppen_provider::models::Models {
        &self.models
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl Run for Acp {
    /// Reopened where it ran, whatever `cwd` says: see `acp_send`.
    fn send<'a>(&'a self, gate: &'a SpawnGate, to: SendTo<'a>) -> BoxFuture<'a, Result<Sent, InjectError>> {
        Box::pin(self.runs.acp_send(gate, to.id, to.text, to.options))
    }

    /// The id is this server's own and the transcript it is kept in is this
    /// instance's, so there is no ownership to record.
    fn start<'a>(&'a self, gate: &'a SpawnGate, at: StartAt<'a>) -> BoxFuture<'a, Result<String, InjectError>> {
        Box::pin(async move {
            let agent = at
                .options
                .acp_agent
                .as_deref()
                .ok_or_else(|| InjectError::message("a new ACP session needs the agent it runs on"))?;
            self.runs.acp_start(gate, agent, at.cwd, at.text, at.options).await
        })
    }

    /// The agent process's own state: ACP runs are not journalled, so the runs
    /// map holds none of them.
    fn injection(&self, id: &str) -> Option<Injection> {
        self.runs.acp_handle(id).and_then(|handle| handle.injection())
    }

    fn resident(&self, id: &str) -> Option<ResidentState> {
        self.runs.acp_handle(id).filter(|h| h.visible()).map(|h| h.state())
    }

    /// Never asked: a schedule cannot start an ACP session, so no task's
    /// previous session is one.
    fn schedule_busy(&self, _id: &str) -> Option<&'static str> {
        None
    }

    // The agent id becomes a path segment, the agent's transcript directory
    // (`AcpTranscripts::agent_dir`), so it is held to `valid_agent_id` before
    // it is looked up; then it has to name a configured agent.
    fn check_start(&self, acp_agent: Option<&str>) -> Result<(), ApiError> {
        match acp_agent {
            None => Err(ApiError::bad_request("an ACP session needs acpAgent: the agent it runs on")),
            Some(id) if !crate::sessions::valid_agent_id(id) => Err(ApiError::bad_request(format!("invalid ACP agent id: {id}"))),
            Some(id) if AcpAgentRegistry::of(&self.runs.host.paths).get(id).is_none() => {
                Err(ApiError::bad_request(format!("no ACP agent \"{id}\" is configured")))
            }
            Some(_) => Ok(()),
        }
    }

    // A session whose agent was removed cannot be continued.
    fn send_block(&self, summary: &SessionSummary) -> Option<String> {
        AcpAgentRegistry::of(&self.runs.host.paths).send_block(summary)
    }

    fn drop_queued<'a>(&'a self, id: &'a str, qid: &'a str) -> BoxFuture<'a, QueueOutcome> {
        Box::pin(self.runs.acp_queue_action(id, qid, false))
    }

    fn steer_queued<'a>(&'a self, id: &'a str, qid: &'a str) -> BoxFuture<'a, QueueOutcome> {
        Box::pin(self.runs.acp_queue_action(id, qid, true))
    }

    /// The agent process, when one is running: an ACP session has no run
    /// outside it.
    fn interrupt(&self, id: &str) -> bool {
        self.runs.acp_handle(id).filter(Handle::alive).is_some_and(|handle| handle.interrupt())
    }

    // The transcript moves out of the agent's list (`archive_acp`).
    fn archive<'a>(&'a self, gate: &'a SpawnGate, id: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move { self.runs.archive_acp(gate, id).await.map_err(|error| error.to_string()) })
    }
}

impl AcpRuns {
    /// ACP's part of the session list's snapshot ([`ProviderRuns::add_to`]): the agent processes
    /// the list is to show, and the state of every one's run.
    pub(crate) fn acp_driver_states(&self, states: &mut DriverStatesBuilder) {
        // Already keyed `key_of`, so the keys go in as they are.
        let acp: Vec<(String, Handle)> = self.acp_servers.lock().unwrap().iter().map(|(key, h)| (key.clone(), h.clone())).collect();
        for (key, handle) in acp.iter().filter(|(_, h)| h.visible()) {
            states.resident(key.clone(), handle.info());
        }
        for (key, handle) in acp {
            if let Some(injection) = handle.injection() {
                states.run(key, injection);
            }
        }
    }

    /// The configuration options `agent` offers, for the composer: the model
    /// list and the effort ladder.
    ///
    /// Remembered per agent id once read — from a probe, or from a session the
    /// agent created since — and forgotten when its settings change. Only a
    /// fresh session's options are remembered: their current values are the
    /// agent's defaults, which a switched session's are not. With
    /// nothing remembered, a probe opens a session in a scratch directory,
    /// reads its options and closes it, so it leaves no conversation behind.
    pub(crate) async fn acp_options(self: &Arc<Self>, agent: &AcpAgent) -> Result<Vec<ConfigOption>, InjectError> {
        if let Some(options) = self.acp_options.lock().unwrap().get(&agent.id) {
            return Ok(options.clone());
        }
        // One probe per agent at a time: a page that asks twice gets the first
        // answer, and a slow agent does not hold up another's list.
        let gate = self.acp_probes.lock().unwrap().entry(agent.id.clone()).or_default().clone();
        let _probing = gate.lock().await;
        if let Some(options) = self.acp_options.lock().unwrap().get(&agent.id) {
            return Ok(options.clone());
        }
        let scratch = scratch("probe").map_err(|error| InjectError::Internal(error.to_string()))?;
        let options = self.probe(agent, &scratch).await?;
        self.remember_acp_options(&agent.id, &options);
        Ok(options)
    }

    async fn probe(&self, agent: &AcpAgent, scratch: &ScratchDir) -> Result<Vec<ConfigOption>, InjectError> {
        let mut started = start_scratch(agent, scratch).await?;
        #[cfg(test)]
        self.host.step("agent probed", Source::Acp, None);
        // Only answers are read: what else the agent says is dropped as it
        // arrives rather than kept.
        started.conn.events.close();
        let opened = tokio::time::timeout(START_TIMEOUT, started.conn.open(None, scratch.path())).await;
        let opened = match opened {
            Ok(Ok(opened)) => opened,
            failed => {
                let reason = match failed {
                    Ok(Err(error)) => error.to_string(),
                    _ => format!("{} did not open a session in time", agent.name),
                };
                return Err(started.fail(self, agent, reason).await);
            }
        };
        // Within a bound: an agent that never answers it is closed regardless.
        let _ = tokio::time::timeout(Duration::from_secs(10), started.conn.close_session(&opened.id)).await;
        started.conn.close().await;
        started.process.end(Duration::from_secs(10)).await;
        Ok(opened.options)
    }

    /// What the agent `agent` offers, as a fresh session reports it.
    pub(crate) fn remember_acp_options(&self, agent: &str, options: &[ConfigOption]) {
        if !options.is_empty() {
            self.acp_options.lock().unwrap().insert(agent.to_string(), options.to_vec());
        }
    }

    /// Forget what the agent `agent` offers: its settings changed, so the next
    /// start may run a different configuration.
    pub(crate) fn forget_acp_options(&self, agent: &str) {
        self.acp_options.lock().unwrap().remove(agent);
    }

    /// Whether an id an agent gave — a model, an effort — can be shown and
    /// sent back as it is: no longer than a name, and nothing in it that
    /// [`droppen_provider::RunHost::redact`] would take out. The agent's own variables'
    /// values are not looked for: they are often what names its model.
    pub(crate) fn acp_id_shown(&self, id: &str) -> bool {
        id.chars().count() <= crate::agents::MAX_AGENT_WORD && self.host.redact(id) == id
    }

    /// What `agent` said, fit to be stored and shown: [`droppen_provider::RunHost::redact`],
    /// after its own variables' values are taken out — they are what an API
    /// key looks like — and then cut to a few KB, as a session's error, its
    /// transcript and a push notification carry it.
    pub(crate) fn redact_acp(&self, agent: &AcpAgent, text: &str) -> String {
        droppen_base::text::clip(&self.host.redact(&agent.without_values(text)), crate::agents::MAX_CHECK_ERROR)
    }

    /// Whether a turn of the agent `agent` is running, in any of its sessions,
    /// or a message is waiting for one.
    fn acp_agent_busy(&self, agent: &str) -> bool {
        self.acp_servers.lock().unwrap().values().any(|handle| handle.runs_on(agent) && handle.busy())
    }

    /// Remove the configured agent `agent` unless a turn of it is running or a
    /// message is waiting for one, which is the `Err`; `Ok(false)` when it was
    /// gone already. Its sessions stay.
    ///
    /// Under the spawn gate, which a start or a send holds from reading its
    /// agent until its message is handed over: no turn starts between the
    /// look and the removal, and none starts on the agent after it.
    pub(crate) async fn remove_acp_agent(&self, gate: &SpawnGate, agent: &AcpAgent) -> Result<bool, String> {
        let _permit = gate.permit().await;
        if self.acp_agent_busy(&agent.id) {
            return Err(format!(
                "{} is running a turn or has a message waiting; stop it, or let it finish, then remove the agent",
                agent.name
            ));
        }
        let removed = AcpAgentRegistry::of(&self.host.paths).remove(&agent.id);
        // An agent configured again under this id is another program, maybe:
        // its sessions start it afresh, and what it offers is asked again.
        self.renew_acp_processes(&agent.id);
        self.forget_acp_options(&agent.id);
        Ok(removed)
    }

    /// Apply `patch` to the agent `id` (adding it when there is none), under
    /// the spawn gate. When its command line or environment changed, its
    /// sessions' processes are renewed ([`AcpRuns::renew_acp_processes`]):
    /// a rotated key, say, reaches every session's next process.
    pub(crate) async fn reconfigure_acp_agent(&self, gate: &SpawnGate, id: &str, patch: &crate::agents::AcpAgentUpdate) -> Result<AcpAgent, String> {
        let _permit = gate.permit().await;
        let agents = AcpAgentRegistry::of(&self.host.paths);
        let before = agents.get(id);
        let after = agents.update(id, patch)?;
        if before.is_none_or(|before| !before.launches_like(&after)) {
            self.renew_acp_processes(id);
        }
        // Another executable or environment may offer other models.
        self.forget_acp_options(id);
        Ok(after)
    }

    /// End the processes `agent`'s sessions run in, so each one's next message
    /// starts the agent as it is configured now. One with nothing to do is
    /// stopped now; one with a turn running, or messages waiting, ends when
    /// they are done. Under the spawn gate, so no message is handed to a
    /// process in between ([`Handle::takes_messages`]).
    fn renew_acp_processes(&self, agent: &str) {
        for handle in self.acp_servers.lock().unwrap().values() {
            if handle.runs_on(agent) && handle.alive() {
                handle.retire();
                if !handle.busy() {
                    // Answered once it is gone; nothing waits for that.
                    let _ = handle.request_stop();
                }
            }
        }
    }

    /// Check `agent`: find its executable on its own `PATH`, start it in a
    /// scratch directory, send `initialize`, read the answer, and stop it.
    /// Nothing reaches a model, and no session is opened.
    ///
    /// Bounded: `initialize` gets [`CHECK_TIMEOUT`], what the agent wrote to
    /// stderr is its last few lines, and the error is cut to a few KB. What it
    /// says is redacted ([`AcpRuns::redact_acp`]).
    pub(crate) async fn check_acp_agent(&self, agent: &AcpAgent) -> AcpCheck {
        self.check_acp_agent_within(agent, CHECK_TIMEOUT).await
    }

    /// [`AcpRuns::check_acp_agent`], waiting `patience` for the answer.
    async fn check_acp_agent_within(&self, agent: &AcpAgent, patience: Duration) -> AcpCheck {
        let env = agent.environment(clean_env_bare());
        let searched = format!("PATH={}", env.get("PATH").map(String::as_str).unwrap_or(""));
        let redact = |text: &str| self.redact_acp(agent, text);
        let Some(resolved) = droppen_base::process_env::resolve_on_path(agent.command.trim(), &env) else {
            let error = format!("{}: no such executable\n{searched}", agent.command.trim());
            return AcpCheck::ended(AcpCheckState::NotFound, Some(redact(&error)));
        };
        let scratch = match scratch("check") {
            Ok(scratch) => scratch,
            Err(error) => {
                return AcpCheck::ended(AcpCheckState::Failed, Some(redact(&format!("cannot make a scratch directory: {error}"))));
            }
        };
        let mut started = match start_scratch(agent, &scratch).await {
            Ok(started) => started,
            Err(error) => return AcpCheck::ended(AcpCheckState::Failed, Some(redact(&format!("{error}\n{searched}")))),
        };
        #[cfg(test)]
        self.host.step("agent checked", Source::Acp, None);
        // Only the answer is read: what else the agent says is dropped as it
        // arrives rather than kept.
        started.conn.events.close();
        let answered = tokio::time::timeout(patience, started.conn.initialize()).await;
        started.client.kill_all();
        started.conn.close().await;
        started.process.end(Duration::from_secs(5)).await;
        match answered {
            Ok(Ok(answer)) => AcpCheck::ready(resolved, &InitializeResponse::read(&answer), redact),
            failed => {
                let reason = match failed {
                    Ok(Err(error)) => error.to_string(),
                    _ => format!("{} did not answer initialize within {}s", agent.name, patience.as_secs()),
                };
                let tail = started.stderr.settled().await;
                let said = if tail.is_empty() { reason } else { format!("{reason}\n{tail}") };
                AcpCheck::ended(AcpCheckState::Failed, Some(redact(&said)))
            }
        }
    }

    pub(crate) fn acp_handle(&self, id: &str) -> Option<Handle> {
        self.acp_servers.lock().unwrap().get(&self.host.key_of(Source::Acp, id)).cloned()
    }

    /// Start a new session on the configured agent `agent` in `cwd`, with its
    /// first message; this server's id for it is returned.
    ///
    /// The spawn gate, which every Claude, Codex and ACP send takes, is held to
    /// read the agent and put the session's handle in the map with the message
    /// on its way, and not while the agent starts or the driver takes the
    /// message: a slow agent, or one that never answers, holds up no other
    /// send.
    pub(crate) async fn acp_start(self: &Arc<Self>, gate: &SpawnGate, agent: &str, cwd: &str, text: &str, options: &SendOptions) -> Result<String, InjectError> {
        // The agent is read under the gate, which its removal holds too
        // ([`AcpRuns::remove_acp_agent`]).
        let mut permit = gate.permit().await;
        let agent = AcpAgentRegistry::of(&self.host.paths)
            .get(agent)
            .ok_or_else(|| InjectError::message(format!("no ACP agent \"{agent}\" is configured; add it in Settings")))?;
        check_options(&agent, options)?;
        let message = message(text, options);
        // Checked before anything starts, against what the agent last offered:
        // a refused pick must not leave a new, empty session behind.
        let offered = self.acp_options.lock().unwrap().get(&agent.id).cloned().unwrap_or_default();
        check_choice(&agent, &offered, message.model.as_deref(), message.effort.as_deref())?;
        let name = agent.name.clone();
        let id = new_uuid();
        let (starting, delivery) = self.acp_starting(&agent, &id, message);
        #[cfg(test)]
        self.host.step("agent placeholder", Source::Acp, Some(&id));
        permit.ungate();
        let open = self.acp_open(&permit, &agent, &id, None, Some(cwd)).await?;
        self.acp_attach(starting, agent, &id, open);
        delivery.taken(&name).await?;
        Ok(id)
    }

    /// Hand a message to the session `id`, starting its own agent if needed:
    /// the one its transcript is kept under, whatever else is configured now.
    /// Under the gate as [`AcpRuns::acp_start`] is.
    pub(crate) async fn acp_send(self: &Arc<Self>, gate: &SpawnGate, id: &str, text: &str, options: &SendOptions) -> Result<Sent, InjectError> {
        let (agent_id, file) = AcpTranscripts::of(&self.host.paths).locate(id)
            .await
            .ok_or_else(|| InjectError::message("ACP session not found"))?;
        let mut permit = gate.permit().await;
        let agent = AcpAgentRegistry::of(&self.host.paths)
            .get(&agent_id)
            .ok_or_else(|| InjectError::message(crate::agents::removed(&agent_id)))?;
        check_options(&agent, options)?;
        let message = message(text, options);
        let current = self.acp_handle(id).filter(Handle::alive);
        // Its configuration changed while it had work: that work ends on the
        // process it started on, and nothing more is given to it. One with
        // nothing left is on its way out, and a new one is started.
        if current.as_ref().is_some_and(|handle| handle.retiring() && handle.busy()) {
            return Err(InjectError::message(format!(
                "{} restarts with its new settings after this turn; send again then",
                agent.name
            )));
        }
        let running = current.filter(Handle::takes_messages);
        // Checked before anything starts, against the session's own options
        // when it is running and what the agent last offered otherwise.
        let offered = match &running {
            Some(handle) => handle.options(),
            None => self.acp_options.lock().unwrap().get(&agent.id).cloned().unwrap_or_default(),
        };
        check_choice(&agent, &offered, message.model.as_deref(), message.effort.as_deref())?;
        let name = agent.name.clone();
        if let Some(handle) = running {
            let delivery = handle.deliver(message);
            permit.ungate();
            return delivery.taken(&name).await;
        }
        let (starting, delivery) = self.acp_starting(&agent, id, message);
        #[cfg(test)]
        self.host.step("agent placeholder", Source::Acp, Some(id));
        permit.ungate();
        // Written by this server when the session was created; a header
        // without the agent's id is from before this server minted its own,
        // when the two were the same.
        let header = read_header(&file).await.unwrap_or_default();
        let recorded = Recorded {
            file,
            agent_session: header.agent_session_id.unwrap_or_else(|| id.to_string()),
            cwd: header.cwd,
        };
        let open = self.acp_open(&permit, &agent, id, Some(recorded), None).await?;
        self.acp_attach(starting, agent, id, open);
        delivery.taken(&name).await
    }

    /// Put the handle of the session `id`, whose agent is about to start, in
    /// the map, with `message` on its way to it: the session reads as starting
    /// meanwhile, with its last turn as the handle it replaces left it, and
    /// its feed and the list are told at once rather than at their next poll.
    /// Under the spawn gate.
    fn acp_starting(&self, agent: &AcpAgent, id: &str, message: Message) -> (Starting<'_>, Delivery) {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = self.host.key_of(Source::Acp, id);
        let mut servers = self.acp_servers.lock().unwrap();
        let handle = Handle::new(tx, &agent.id, servers.get(&key));
        let delivery = handle.deliver(message);
        let replaced = servers.insert(key, handle.clone());
        drop(servers);
        self.host.announce(Source::Acp, id);
        let starting = Starting {
            runs: self,
            id: id.to_string(),
            handle,
            rx: Some(rx),
            replaced,
        };
        (starting, delivery)
    }

    pub(crate) async fn acp_queue_action(&self, id: &str, qid: &str, steer: bool) -> QueueOutcome {
        match self.acp_handle(id).filter(Handle::alive) {
            Some(handle) => handle.queue_action(qid, steer).await,
            None => QueueOutcome::NoResident,
        }
    }

    /// Put an ACP session away: its transcript leaves the list, and the
    /// agent's own record of it is left alone. A session of an agent that was
    /// removed can be put away too.
    ///
    /// Refused while a turn is running or a message is waiting, as the other
    /// archives are: the user is told rather than the work being dropped.
    pub(crate) async fn archive_acp(&self, gate: &SpawnGate, id: &str) -> Result<(), droppen_sessions::archive::ArchiveError> {
        use droppen_sessions::archive::ArchiveError;
        let (agent_id, from) = AcpTranscripts::of(&self.host.paths).locate(id)
            .await
            .ok_or_else(|| ArchiveError("cannot archive the ACP session: it is gone".to_string()))?;
        let name = AcpAgentRegistry::of(&self.host.paths).get(&agent_id).map_or(agent_id.clone(), |agent| agent.name);
        let busy = || {
            ArchiveError(format!(
                "{name} is running or has queued messages; stop it and clear the queue before archiving"
            ))
        };
        // The stop is asked for under the gate and waited for outside it, as a
        // send is; the move is made under it again, with nothing running.
        let permit = gate.permit().await;
        let stopping = match self.acp_handle(id).filter(Handle::alive) {
            Some(handle) if handle.busy() => return Err(busy()),
            Some(handle) => handle.request_stop(),
            None => None,
        };
        drop(permit);
        #[cfg(test)]
        self.host.step("agent stopping", Source::Acp, Some(id));
        if let Some(stopping) = stopping {
            // No answer: the driver is gone, which is stopped too.
            if !stopping.await.unwrap_or(true) {
                return Err(busy());
            }
        }
        let _permit = gate.permit().await;
        if self.acp_handle(id).is_some_and(|handle| handle.alive()) {
            // A message started it again meanwhile.
            return Err(busy());
        }
        self.acp_servers.lock().unwrap().remove(&self.host.key_of(Source::Acp, id));
        let archived = from.with_file_name("archived");
        tokio::fs::create_dir_all(&archived)
            .await
            .and(tokio::fs::rename(&from, archived.join(format!("{id}.jsonl"))).await)
            .map_err(|error| ArchiveError(format!("cannot archive the ACP session: {error}")))
    }

    /// Start `agent` for the session `id` and open it: a new one in `cwd`, or
    /// the `recorded` one, reopened where it ran with `session/load`. Not under
    /// the spawn gate: this is what can take as long as [`START_TIMEOUT`].
    async fn acp_open(&self, permit: &SpawnPermit<'_>, agent: &AcpAgent, id: &str, recorded: Option<Recorded>, cwd: Option<&str>) -> Result<Open, InjectError> {
        // A session that exists ran somewhere; that is where it is reopened,
        // whatever directory the request names.
        let workdir = session_workdir(&self.host.paths, recorded.as_ref().and_then(|recorded| recorded.cwd.as_deref()).or(cwd));
        let mut started = start(agent, &workdir, permit).await?;
        #[cfg(test)]
        self.host.step("agent started", Source::Acp, Some(id));
        let reopen = recorded.as_ref().map(|recorded| recorded.agent_session.as_str());
        let opened = match tokio::time::timeout(START_TIMEOUT, started.conn.open(reopen, &workdir)).await {
            Ok(Ok(opened)) => opened,
            failed => {
                let reason = match failed {
                    Ok(Err(error)) => error.to_string(),
                    _ => format!("{} did not open the session in time", agent.name),
                };
                return Err(started.fail(self, agent, reason).await);
            }
        };
        let file = match recorded {
            Some(recorded) => recorded.file,
            None => {
                // A new session's options say what a fresh one starts on — the
                // defaults the composer offers. A reopened one's say what *it*
                // was switched to, which is nobody else's default.
                self.remember_acp_options(&agent.id, &opened.options);
                let dir = AcpTranscripts::of(&self.host.paths).agent_dir(&agent.id).ok_or_else(|| InjectError::Internal(format!("unusable agent id {}", agent.id)))?;
                tokio::fs::create_dir_all(&dir)
                    .await
                    .map_err(|error| InjectError::Internal(error.to_string()))?;
                dir.join(format!("{id}.jsonl"))
            }
        };

        let fresh = tokio::fs::metadata(&file).await.map(|metadata| metadata.len() == 0).unwrap_or(true);
        let mut transcript = driver::Transcript::open(&file)
            .await
            .map_err(|error| InjectError::Internal(error.to_string()))?;
        if fresh {
            transcript
                .append(&[Record::Session {
                    id,
                    agent: &agent.id,
                    agent_session_id: &opened.id,
                    cwd: &workdir.to_string_lossy(),
                }
                .stamped()])
                .await;
        }
        // The agent's own history, for a transcript this server does not have:
        // the session is one the agent kept and this server lost, or never
        // wrote down.
        if !has_conversation(&file).await {
            let mut translator = Translator::default();
            let mut records: Vec<Value> = opened.replay.iter().flat_map(|update| translator.replayed(update)).collect();
            records.extend(translator.flush());
            transcript.append(&records).await;
        }
        Ok(Open {
            agent_session: opened.id,
            started,
            transcript,
            options: opened.options,
        })
    }

    /// Give the session `id`'s started agent its driver, on the handle
    /// `starting` put in the map. The messages on their way are in its channel
    /// already, so the driver takes them first; one whose agent was edited
    /// meanwhile is retiring, and ends once it has run them.
    fn acp_attach(self: &Arc<Self>, mut starting: Starting<'_>, agent: AcpAgent, id: &str, open: Open) {
        let rx = starting.rx.take().expect("a session is attached once");
        let (in_flight, retire) = starting.handle.counters();
        let Open {
            agent_session,
            started: Started { process, stderr, client, conn },
            transcript,
            options,
        } = open;
        let driver = Driver {
            runs: self.clone(),
            agent,
            id: id.to_string(),
            agent_session,
            process,
            conn,
            client,
            stderr,
            transcript,
            translator: Translator::default(),
            state: State::opened(options, starting.handle.injection()),
            shared: starting.handle.shared(),
            rx,
            in_flight,
            retire,
            interrupted: false,
            recorded_choice: None,
        };
        tokio::spawn(driver.run());
    }
}

/// The working directory as ACP wants it: absolute.
fn absolute(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// What a session's replay is made of, for the caller to translate.
pub(crate) type Replay = Vec<crate::protocol::SessionUpdate>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::tests::unstamped;
    use droppen_instance::paths::Paths;
    use serde_json::json;

    #[test]
    fn attachments_reach_the_agent_as_paths() {
        assert_eq!(prompt_text("hi", &[], &[]), "hi");
        let prompt = prompt_text("look", &["/u/a.png".into()], &["/u/b.txt".into()]);
        assert_eq!(prompt, "look\n\n[2 attached file(s):]\n- /u/a.png\n- /u/b.txt");
    }

    /// The ACP provider and no other, on the base's own injection registry
    /// at a home ([`droppen_provider::testing::injection_registry`]: stand-in
    /// logins, spawns decorated with nothing). No server is built.
    struct AcpAlone {
        /// The home's paths, where the agents' settings and transcripts are.
        paths: Arc<Paths>,
        /// The registry ACP is registered in, which lends its spawn gate.
        providers: Arc<droppen_provider::ProviderRegistry>,
    }

    /// The ACP provider, registered alone.
    fn acp_only(providers: &mut droppen_provider::ProviderRegistry, registry: &Arc<InjectionRegistry>) {
        providers.register(Arc::new(Acp::new(registry.clone())));
    }

    /// [`AcpAlone`] at `home`.
    fn acp_alone(home: &Path) -> AcpAlone {
        let injections = droppen_provider::testing::injection_registry(home);
        let paths = injections.host.paths.clone();
        let providers = droppen_provider::ProviderRegistry::new(injections, acp_only);
        AcpAlone { paths, providers }
    }

    /// The ACP runs of `state`'s provider.
    fn acp_of(state: &AcpAlone) -> Arc<AcpRuns> {
        state.providers.provider::<Acp>().expect("the ACP provider").runs().clone()
    }

    fn agent() -> AcpAgent {
        AcpAgent {
            id: "fake".into(),
            name: "Fake agent".into(),
            command: "/x/fake".into(),
            args: vec![],
            env: vec![],
            last_check: None,
        }
    }

    #[test]
    fn options_an_acp_agent_cannot_honour_are_refused() {
        let agent = &agent();
        assert!(check_options(agent, &SendOptions::default()).is_ok());
        assert!(check_options(
            agent,
            &SendOptions {
                account: AccountChoice::Local,
                ..SendOptions::default()
            }
        )
        .is_ok());
        for refused in [
            SendOptions {
                account: AccountChoice::Account("a".into()),
                ..SendOptions::default()
            },
            SendOptions {
                permission: Some("auto".into()),
                ..SendOptions::default()
            },
            SendOptions {
                chat: true,
                ..SendOptions::default()
            },
        ] {
            let error = check_options(agent, &refused).expect_err("refused");
            assert!(error.to_string().contains("Fake agent"), "the refusal names the agent: {error}");
        }
    }

    #[test]
    fn a_model_or_effort_is_checked_against_what_the_agent_offers() {
        let agent = &agent();
        let offered = crate::protocol::config_options(&json!({"configOptions": [
            {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "a:x",
             "options": [{"value": "a:x", "name": "X"}, {"value": "a:y", "name": "Y"}]},
            {"id": "reasoning_effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": "auto",
             "options": [{"value": "auto", "name": "Auto"}, {"value": "high", "name": "High"}]},
        ]}));
        assert!(check_choice(agent, &offered, Some("a:y"), Some("high")).is_ok());
        assert!(check_choice(agent, &offered, None, None).is_ok());
        assert!(check_choice(agent, &offered, Some("a:z"), None).is_err());
        assert!(check_choice(agent, &offered, None, Some("ultra")).is_err());
        assert!(
            check_choice(agent, &[], Some("anything"), Some("goes")).is_ok(),
            "unknown yet: the agent answers"
        );
        let model_only = &offered[..1];
        let refused = check_choice(agent, model_only, None, Some("high")).expect_err("no effort to choose");
        assert!(refused.to_string().contains("offers no effort"), "{refused}");
    }

    /// A stand-in that never answers and is deaf to its input: it writes down
    /// where it runs and the pid of a helper it forks into `record`, then
    /// sleeps.
    fn deaf_agent(record: &Path) -> AcpAgent {
        use std::os::unix::fs::PermissionsExt;
        let program = record.join("deaf-agent");
        std::fs::write(
            &program,
            "#!/bin/sh\npwd > \"$RECORD/cwd\"\nsleep 300 &\necho $! > \"$RECORD/helper\"\nexec sleep 300\n",
        )
        .expect("agent");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        AcpAgent {
            command: program.to_string_lossy().into_owned(),
            env: vec![droppen_base::env_block::EnvVar {
                key: "RECORD".into(),
                value: record.to_string_lossy().into_owned(),
            }],
            ..agent()
        }
    }

    /// Where the stand-in ran, and its helper's pid.
    fn recorded(record: &Path) -> (PathBuf, u32) {
        let read = |name: &str| std::fs::read_to_string(record.join(name)).unwrap_or_else(|error| panic!("{name}: {error}"));
        (PathBuf::from(read("cwd").trim()), read("helper").trim().parse().expect("a pid"))
    }

    /// Wait for `pid` to be gone: killed. A zombie counts: the process it was
    /// left to may never reap it, as a container's pid 1 need not.
    async fn gone(pid: u32) {
        let zombie = || {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|stat| stat.rsplit_once(')').map(|(_, rest)| rest.trim_start().starts_with('Z')))
                .unwrap_or(false)
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while droppen_base::procs::alive(pid) && !zombie() {
            assert!(std::time::Instant::now() < deadline, "the helper {pid} is still running");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Wait for `what` to hold, polling.
    async fn until(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !condition() {
            assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn an_agent_that_exits_on_its_own_takes_what_it_started_with_it() {
        let record = tempfile::tempdir().expect("tempdir");
        let helper = record.path().join("helper");
        let mut child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "sleep 300 & echo $! > \"$1\"; exec cat >/dev/null", "sh"])
            .arg(&helper)
            .stdin(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .expect("spawn");
        let stdin = child.stdin.take();
        let mut process = Process::new(child);
        until("the helper", || std::fs::read_to_string(&helper).is_ok_and(|pid| pid.ends_with('\n'))).await;
        let pid: u32 = std::fs::read_to_string(&helper).unwrap().trim().parse().expect("a pid");
        // Its input closes, and it exits: what it left running goes too.
        drop(stdin);
        let ending = std::time::Instant::now();
        process.end(Duration::from_secs(10)).await;
        assert!(ending.elapsed() < Duration::from_secs(5), "its exit is seen, not waited out");
        gone(pid).await;
    }

    #[test]
    fn a_message_on_its_way_to_the_driver_keeps_the_session_busy() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = Handle::new(tx, "a", None);
        assert!(!handle.busy());
        let delivery = handle.deliver(message("hi", &SendOptions::default()));
        assert!(handle.busy(), "sent, not taken yet");
        drop(delivery);
        assert!(handle.busy(), "its sender gave up waiting; the message is still on its way");
        let taken = rx.try_recv();
        assert!(matches!(taken, Ok(driver::Cmd::Deliver(..))));
        drop(taken);
        assert!(!handle.busy(), "taken, or dropped with the driver");
    }

    /// Configure the stand-in ACP agent the integration tests run as `id`,
    /// with the variables `env`.
    fn configure_fake(paths: &Paths, home: &Path, id: &str, env: &str) {
        use std::os::unix::fs::PermissionsExt;
        let program = home.join("fake-agent");
        std::fs::write(&program, include_str!("../../../tests/support/fake_acp_agent.py")).expect("agent");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let patch = crate::agents::AcpAgentUpdate {
            name: None.into(),
            command: Some(program.to_string_lossy().into_owned()).into(),
            args: Some(vec!["--acp".to_string(), "--stdio".to_string()]).into(),
            env: Some(env.to_string()).into(),
        };
        AcpAgentRegistry::of(paths).update(id, &patch).expect("configured");
    }

    /// Whether the stand-in in `workdir` was sent `method`.
    fn was_sent(workdir: &Path, method: &str) -> bool {
        std::fs::read_to_string(workdir.join("agent-calls.jsonl")).is_ok_and(|calls| calls.contains(&format!("\"method\": \"{method}\"")))
    }

    /// Workdirs under `home`, one per name.
    fn workdirs(home: &Path, names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| {
                let dir = home.join(name);
                std::fs::create_dir_all(&dir).expect("workdir");
                dir
            })
            .collect()
    }

    /// An agent that never answers a model switch holds up its own session
    /// only: a message sent to it waits outside the spawn gate, so another
    /// agent's session starts meanwhile. The switch is given up on after its
    /// timeout: its message fails, the process goes, and so do the message
    /// queued behind it and the one on its way — with the error written down
    /// once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_agent_that_never_answers_a_model_switch_holds_up_no_other_send() {
        let home = tempfile::tempdir().expect("tempdir");
        let state = acp_alone(home.path());
        let injections = acp_of(&state);
        let gate = state.providers.gate();
        configure_fake(&state.paths, home.path(), "stuck", "FAKE_AGENT_SLOW=session/set_config_option=600");
        configure_fake(&state.paths, home.path(), "other", "");
        let dirs = workdirs(home.path(), &["stuck", "other"]);
        let (stuck_dir, other_dir) = (&dirs[0], &dirs[1]);
        let plain = SendOptions::default();

        let id = injections.acp_start(gate, "stuck", &stuck_dir.to_string_lossy(), "hello", &plain).await.expect("started");
        until("the first turn", || injections.acp_handle(&id).is_some_and(|handle| !handle.busy())).await;
        // A turn that waits, with the switch and one more message queued behind it.
        injections.acp_send(gate, &id, "wait for me", &plain).await.expect("handed over");
        let running = || {
            injections
                .acp_handle(&id)
                .and_then(|handle| handle.injection())
                .is_some_and(|injection| injection.state == droppen_sessions::InjectionRunState::Running)
        };
        until("the long turn", running).await;
        let switch = SendOptions {
            model: Some("openrouter:vendor/big".into()),
            ..SendOptions::default()
        };
        assert!(injections.acp_send(gate, &id, "switch", &switch).await.expect("handed over").queued);
        assert!(injections.acp_send(gate, &id, "queued behind", &plain).await.expect("handed over").queued);
        assert!(injections.acp_handle(&id).expect("a handle").interrupt());
        until("the switch", || was_sent(stuck_dir, "session/set_config_option")).await;

        // The driver is stuck on the switch, so the next message waits for it…
        let behind = tokio::spawn({
            let (injections, providers, id) = (injections.clone(), state.providers.clone(), id.clone());
            async move { injections.acp_send(providers.gate(), &id, "behind it", &SendOptions::default()).await }
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!behind.is_finished(), "it waits for the driver");
        // …and nobody else does.
        let other = injections.acp_start(gate, "other", &other_dir.to_string_lossy(), "hi", &plain).await;
        assert!(other.is_ok(), "{:?}", other.err());
        assert!(!behind.is_finished(), "another agent's session started while the switch was still unanswered");

        // Given up on: the message fails, the process goes, and so does what
        // was waiting for it.
        let behind = tokio::time::timeout(Duration::from_secs(20), behind).await.expect("answered").expect("joined");
        assert!(behind.is_err(), "the process went before it took the message");
        until("the process to go", || injections.acp_handle(&id).is_some_and(|handle| !handle.alive())).await;
        let (_, file) = AcpTranscripts::of(&state.paths).locate(&id).await.expect("its transcript");
        let transcript = std::fs::read_to_string(file).expect("transcript");
        assert_eq!(transcript.matches("did not answer a switch of its model").count(), 1, "{transcript}");
        let said = transcript.lines().find(|line| line.contains("did not answer a switch")).expect("the error");
        assert_eq!(
            unstamped(said),
            r#"{"message":"stuck did not answer a switch of its model to openrouter:vendor/big within 5s; it was stopped","type":"error","timestamp":"<timestamp>"}"#
        );
    }

    /// An agent that hangs while it starts holds up nothing but its own
    /// session: the spawn gate, which every Claude and Codex send takes too, is
    /// free meanwhile, another agent's session starts and is sent to, and the
    /// starting agent is not removed under it. Given up on, the start leaves
    /// nothing behind.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_agent_that_hangs_while_it_starts_holds_up_no_other_send() {
        let home = tempfile::tempdir().expect("tempdir");
        let state = acp_alone(home.path());
        let injections = acp_of(&state);
        let gate = state.providers.gate();
        configure_fake(&state.paths, home.path(), "hangs", "FAKE_AGENT_SLOW=initialize=600");
        configure_fake(&state.paths, home.path(), "other", "");
        let dirs = workdirs(home.path(), &["hangs", "other"]);
        let plain = SendOptions::default();

        let starting = tokio::spawn({
            let (injections, providers, dir) = (injections.clone(), state.providers.clone(), dirs[0].to_string_lossy().into_owned());
            async move { injections.acp_start(providers.gate(), "hangs", &dir, "hello", &SendOptions::default()).await }
        });
        until("initialize", || was_sent(&dirs[0], "initialize")).await;

        let quickly = Duration::from_secs(10);
        let permit = tokio::time::timeout(Duration::from_secs(1), gate.permit()).await;
        assert!(permit.is_ok(), "the spawn gate is held while an agent starts");
        drop(permit);
        let other = tokio::time::timeout(quickly, injections.acp_start(gate, "other", &dirs[1].to_string_lossy(), "hi", &plain))
            .await
            .expect("not held up")
            .expect("started");
        until("the other turn", || injections.acp_handle(&other).is_some_and(|handle| !handle.busy())).await;
        tokio::time::timeout(quickly, injections.acp_send(gate, &other, "again", &plain))
            .await
            .expect("not held up")
            .expect("sent");

        let hangs = AcpAgentRegistry::of(&state.paths).get("hangs").expect("configured");
        let refused = injections.remove_acp_agent(gate, &hangs).await.expect_err("it is starting");
        assert!(refused.contains("running a turn or has a message waiting"), "{refused}");
        assert!(!starting.is_finished());

        starting.abort();
        let _ = starting.await;
        assert!(!injections.acp_agent_busy("hangs"), "nothing of the start is left");
        assert_eq!(injections.remove_acp_agent(gate, &hangs).await, Ok(true));
    }

    /// A stop asked for while a message is on its way to the driver is
    /// refused, and the message is taken: an archive racing a send loses.
    #[tokio::test]
    async fn a_stop_does_not_drop_a_message_on_its_way() {
        let home = tempfile::tempdir().expect("tempdir");
        let state = acp_alone(home.path());
        let injections = acp_of(&state);
        let gate = state.providers.gate();
        configure_fake(&state.paths, home.path(), "fake", "");
        let dir = workdirs(home.path(), &["work"]).remove(0);
        let id = injections
            .acp_start(gate, "fake", &dir.to_string_lossy(), "hello", &SendOptions::default())
            .await
            .expect("started");
        until("the first turn", || injections.acp_handle(&id).is_some_and(|handle| !handle.busy())).await;

        // One thread: the driver reads both only once this test waits.
        let handle = injections.acp_handle(&id).expect("a handle");
        let stopping = handle.request_stop().expect("a driver");
        let delivery = handle.deliver(message("after", &SendOptions::default()));
        assert!(!stopping.await.expect("answered"), "a message was on its way");
        assert!(delivery.taken("Fake").await.is_ok(), "and it was taken");
    }

    /// Every line of the session `id`'s transcript, unstamped: the bytes this
    /// and other versions read back.
    async fn transcript_lines(paths: &Paths, id: &str) -> Vec<String> {
        let (_, file) = AcpTranscripts::of(paths).locate(id).await.expect("its transcript");
        let text = std::fs::read_to_string(file).expect("transcript");
        assert!(text.ends_with('\n'), "every line is ended: {text}");
        text.lines().map(unstamped).collect()
    }

    /// A session's agent process is in the snapshot its driver hands a turn's
    /// alert ([`RunHost::driver_states_of`]), idle as the turn left it, as
    /// well as in the one the session list asks the providers for.
    #[tokio::test]
    async fn an_acp_session_is_in_its_turn_alert_s_snapshot() {
        let home = tempfile::tempdir().expect("tempdir");
        let registry = droppen_provider::testing::injection_registry(home.path());
        let paths = registry.host.paths.clone();
        let alerts = Arc::new(droppen_provider::testing::TurnAlerts::default());
        registry.host.attach_alerts(alerts.clone());
        let providers = droppen_provider::ProviderRegistry::new(registry, acp_only);
        configure_fake(&paths, home.path(), "fake", "");
        let dir = workdirs(home.path(), &["work"]).remove(0);
        let runs = providers.provider::<Acp>().expect("the ACP provider").runs().clone();
        let id = runs.acp_start(providers.gate(), "fake", &dir.to_string_lossy(), "hi", &SendOptions::default()).await.expect("started");
        until("the turn's alert", || !alerts.seen().is_empty()).await;
        let (source, alerted, resident) = alerts.seen().remove(0);
        assert_eq!((source, alerted.as_str()), (Source::Acp, id.as_str()));
        assert_eq!(resident.map(|resident| resident.phase), Some(droppen_sessions::ResidentPhase::Idle), "its agent process, as the turn left it");
        let listed = providers.driver_states().resident(Source::Acp, &id).expect("in the providers' snapshot");
        assert_eq!(listed.phase, droppen_sessions::ResidentPhase::Idle);
    }

    /// What the sessions' feeds and the session list were told, in order: a
    /// feed's poke by its key, and a list's refresh by the session it names.
    #[derive(Default)]
    struct Told(Mutex<Vec<String>>);

    impl Told {
        /// Everything told since the last call.
        fn take(&self) -> Vec<String> {
            std::mem::take(&mut self.0.lock().unwrap())
        }
    }

    impl droppen_provider::SessionFeeds for Told {
        fn poke(&self, key: &str) {
            self.0.lock().unwrap().push(format!("feed {key}"));
        }
    }

    impl droppen_http::refresh::RefreshSink for Told {
        fn request_refresh(&self, expected: Option<(&dyn std::fmt::Display, &str)>) {
            let named = expected.map(|(source, id)| format!(" {source}/{id}")).unwrap_or_default();
            self.0.lock().unwrap().push(format!("list{named}"));
        }

        fn publish(&self, _frame: String) {}
    }

    /// A session whose agent process is gone reads as working while the agent
    /// starts again for its next message: the process, then `session/load`,
    /// held here. Its feed and the list are told at once, not at their next
    /// poll. Meanwhile the session still says how its last turn ended, as it
    /// did before the message was sent; a start given up on puts that back as
    /// it was, and says so too.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_reads_as_working_while_its_agent_restarts_for_a_message() {
        let home = tempfile::tempdir().expect("tempdir");
        let (state, id, dir) = one_turn(home.path(), "", "hello", &SendOptions::default()).await;
        let injections = acp_of(&state);
        let gate = state.providers.gate();
        let told = Arc::new(Told::default());
        injections
            .host
            .attach_listeners(told.clone(), droppen_http::refresh::Refresh::new(told.clone()));
        let announced = [
            format!("feed {}", injections.host.key_of(Source::Acp, &id)),
            format!("list {}/{id}", Source::Acp),
        ];
        let shown = |injection: Option<Injection>| injection.map(|injection| (injection.state, injection.started_at, injection.finished_at.0));
        let last = shown(state.providers.injection_for(Source::Acp, &id));
        assert_eq!(
            last.as_ref().map(|last| last.0),
            Some(droppen_sessions::InjectionRunState::Ok),
            "the first turn ended"
        );

        // A changed environment ends the idle process, so the next message
        // starts the agent again: one that holds `session/load`.
        let slow = crate::agents::AcpAgentUpdate {
            name: None.into(),
            command: None.into(),
            args: None.into(),
            env: Some("FAKE_AGENT_SLOW=session/load=600".to_string()).into(),
        };
        injections.reconfigure_acp_agent(gate, "fake", &slow).await.expect("reconfigured");
        until("the process to go", || injections.acp_handle(&id).is_some_and(|handle| !handle.alive())).await;
        told.take();
        let sending = tokio::spawn({
            let (injections, providers, id) = (injections.clone(), state.providers.clone(), id.clone());
            async move { injections.acp_send(providers.gate(), &id, "again", &SendOptions::default()).await }
        });
        until("session/load", || was_sent(&dir, "session/load")).await;
        assert_eq!(told.take(), announced, "the starting agent is announced");

        let resident = state.providers.resident_info(Source::Acp, &id).expect("the starting agent is on view");
        assert_eq!(resident.phase, droppen_sessions::ResidentPhase::Starting);
        assert_eq!(
            resident.activity(),
            droppen_sessions::SessionActivity::Working,
            "at work on the message, not idle"
        );
        assert_eq!(
            shown(state.providers.injection_for(Source::Acp, &id)),
            last,
            "the last turn stays on view"
        );
        let listed = state
            .providers
            .driver_states()
            .resident(Source::Acp, &id)
            .expect("in the providers' snapshot");
        assert_eq!(listed.phase, droppen_sessions::ResidentPhase::Starting);
        assert!(!sending.is_finished(), "the message waits for the agent");

        sending.abort();
        let _ = sending.await;
        assert_eq!(told.take(), announced, "so is the start given up on");
        assert!(state.providers.resident_info(Source::Acp, &id).is_none(), "no agent process is left");
        assert_eq!(
            shown(state.providers.injection_for(Source::Acp, &id)),
            last,
            "the session is as the start found it"
        );
    }

    /// The last turn stays on view until the next one starts, on an agent
    /// started again for the message as on one that was running: the driver
    /// opens with what its handle showed. Held here at the switch to the model
    /// the message asks for, after the driver took it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_restarted_agent_keeps_the_last_turn_on_view_until_it_runs_the_next() {
        let home = tempfile::tempdir().expect("tempdir");
        let (state, id, dir) = one_turn(home.path(), "", "hello", &SendOptions::default()).await;
        let injections = acp_of(&state);
        let gate = state.providers.gate();
        let shown = |injection: Option<Injection>| injection.map(|injection| (injection.state, injection.started_at, injection.finished_at.0));
        let last = shown(state.providers.injection_for(Source::Acp, &id));
        assert!(last.is_some(), "the first turn ended");

        let slow = crate::agents::AcpAgentUpdate {
            name: None.into(),
            command: None.into(),
            args: None.into(),
            env: Some("FAKE_AGENT_SLOW=session/set_config_option=600".to_string()).into(),
        };
        injections.reconfigure_acp_agent(gate, "fake", &slow).await.expect("reconfigured");
        until("the process to go", || injections.acp_handle(&id).is_some_and(|handle| !handle.alive())).await;
        let switch = SendOptions {
            model: Some("openrouter:vendor/big".into()),
            ..SendOptions::default()
        };
        injections.acp_send(gate, &id, "again", &switch).await.expect("taken");
        until("the switch", || was_sent(&dir, "session/set_config_option")).await;

        assert_eq!(
            shown(state.providers.injection_for(Source::Acp, &id)),
            last,
            "the last turn stays on view"
        );
    }

    /// A Stop while a session's agent starts again for a message is taken,
    /// not refused: it waits behind the message, and the driver cancels the
    /// turn the message starts as soon as it starts it. The message is one
    /// the stand-in holds open until it is cancelled.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stop_while_the_agent_restarts_ends_the_turn_it_starts() {
        let home = tempfile::tempdir().expect("tempdir");
        let (state, id, dir) = one_turn(home.path(), "", "hello", &SendOptions::default()).await;
        let injections = acp_of(&state);
        let gate = state.providers.gate();
        let first = state.providers.injection_for(Source::Acp, &id).expect("the first turn's").started_at;

        // Held at `session/load` long enough to be stopped meanwhile.
        let slow = crate::agents::AcpAgentUpdate {
            name: None.into(),
            command: None.into(),
            args: None.into(),
            env: Some("FAKE_AGENT_SLOW=session/load=3".to_string()).into(),
        };
        injections.reconfigure_acp_agent(gate, "fake", &slow).await.expect("reconfigured");
        until("the process to go", || injections.acp_handle(&id).is_some_and(|handle| !handle.alive())).await;
        let sending = tokio::spawn({
            let (injections, providers, id) = (injections.clone(), state.providers.clone(), id.clone());
            async move { injections.acp_send(providers.gate(), &id, "wait for me", &SendOptions::default()).await }
        });
        until("session/load", || was_sent(&dir, "session/load")).await;
        let resident = state.providers.resident_info(Source::Acp, &id).expect("the starting agent is on view");
        assert_eq!(resident.phase, droppen_sessions::ResidentPhase::Starting);
        assert!(
            state.providers.interrupt(Source::Acp, &id),
            "a Stop during the start is taken, not refused"
        );

        sending.await.expect("joined").expect("the message is taken");
        let ended = || {
            state
                .providers
                .injection_for(Source::Acp, &id)
                .filter(|injection| injection.started_at != first && injection.state != droppen_sessions::InjectionRunState::Running)
        };
        until("the turn to end", || ended().is_some()).await;
        let ended = ended().expect("ended");
        assert!(ended.interrupted, "the turn ends as stopped: {ended:?}");
        assert!(was_sent(&dir, "session/cancel"), "the agent was told to cancel it");
    }

    /// A Stop names the turn it means, and stops no other. Two messages are
    /// sent while the agent starts, then a Stop, then a third. The first turn
    /// is over before the driver reads the Stop: the agent has no model by
    /// the name it asks for, so it ends at once. The Stop then leaves the
    /// second turn running. The third message is taken only once the driver
    /// has read the Stop. A Stop sent now names the second turn, and stops it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stop_does_not_cancel_the_turn_after_the_one_it_meant() {
        let home = tempfile::tempdir().expect("tempdir");
        let (state, id, dir) = one_turn(home.path(), "", "hello", &SendOptions::default()).await;
        let injections = acp_of(&state);
        let gate = state.providers.gate();
        let slow = crate::agents::AcpAgentUpdate {
            name: None.into(),
            command: None.into(),
            args: None.into(),
            env: Some("FAKE_AGENT_SLOW=session/load=3".to_string()).into(),
        };
        injections.reconfigure_acp_agent(gate, "fake", &slow).await.expect("reconfigured");
        until("the process to go", || injections.acp_handle(&id).is_some_and(|handle| !handle.alive())).await;
        let send = |text: &'static str, model: Option<&'static str>| {
            let (injections, providers, id) = (injections.clone(), state.providers.clone(), id.clone());
            tokio::spawn(async move {
                let options = SendOptions {
                    model: model.map(str::to_owned),
                    ..SendOptions::default()
                };
                injections.acp_send(providers.gate(), &id, text, &options).await
            })
        };
        let refused = send("first", Some("no-such-model"));
        until("session/load", || was_sent(&dir, "session/load")).await;
        let held = send("wait for me", None);
        let on_their_way = || {
            injections
                .acp_handle(&id)
                .map_or(0, |handle| handle.counters().0.load(std::sync::atomic::Ordering::Acquire))
        };
        until("the second message on its way", || on_their_way() == 2).await;
        let resident = state.providers.resident_info(Source::Acp, &id).expect("the starting agent is on view");
        assert_eq!(resident.phase, droppen_sessions::ResidentPhase::Starting);
        assert!(state.providers.interrupt(Source::Acp, &id), "a Stop during the start is taken");
        let after = send("after the stop", None);

        refused.await.expect("joined").expect("taken");
        held.await.expect("joined").expect("taken");
        assert!(after.await.expect("joined").expect("taken").queued, "behind the second turn");
        let second = state.providers.injection_for(Source::Acp, &id).expect("the second turn's");
        assert_eq!(second.state, droppen_sessions::InjectionRunState::Running, "the second turn runs");
        // A cancel the driver sent is in the agent's call log within a
        // moment; a second is far longer than that.
        let quiet = std::time::Instant::now() + Duration::from_secs(1);
        while std::time::Instant::now() < quiet {
            assert!(!was_sent(&dir, "session/cancel"), "the Stop meant the first turn, not the second");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let still = state.providers.injection_for(Source::Acp, &id).expect("the second turn's");
        assert_eq!((still.state, still.started_at), (second.state, second.started_at), "it runs on");

        assert!(state.providers.interrupt(Source::Acp, &id), "a Stop now names the second turn");
        until("the second turn's cancel", || was_sent(&dir, "session/cancel")).await;
    }

    /// A message whose model the agent is switched to first reads as starting
    /// while the switch runs: the driver has taken it off the queue, so it is
    /// neither queued nor idle, and the session stays busy. A Stop meanwhile is
    /// taken, not refused, and stops the turn the message starts. Held here at
    /// `session/set_config_option`, for a message the stand-in holds open until
    /// it is cancelled.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_message_reads_as_starting_while_its_model_is_switched() {
        let home = tempfile::tempdir().expect("tempdir");
        let (state, id, dir) = one_turn(
            home.path(),
            "FAKE_AGENT_SLOW=session/set_config_option=3",
            "hello",
            &SendOptions::default(),
        )
        .await;
        let injections = acp_of(&state);
        let switch = SendOptions {
            model: Some("openrouter:vendor/big".into()),
            ..SendOptions::default()
        };
        injections
            .acp_send(state.providers.gate(), &id, "wait for me", &switch)
            .await
            .expect("taken");
        until("the switch", || was_sent(&dir, "session/set_config_option")).await;

        let resident = state.providers.resident_info(Source::Acp, &id).expect("its agent process");
        assert_eq!(resident.phase, droppen_sessions::ResidentPhase::Starting);
        assert_eq!(
            resident.activity(),
            droppen_sessions::SessionActivity::Working,
            "at work on the message, not idle"
        );
        assert_eq!(resident.queued, 0, "the message is off the queue");
        assert!(injections.acp_handle(&id).expect("its handle").busy(), "the session is busy");
        assert!(
            state.providers.interrupt(Source::Acp, &id),
            "a Stop during the switch is taken, not refused"
        );

        until("the turn's cancel", || was_sent(&dir, "session/cancel")).await;
        let ended = || {
            state
                .providers
                .injection_for(Source::Acp, &id)
                .filter(|injection| injection.state != droppen_sessions::InjectionRunState::Running && injection.interrupted)
        };
        until("the turn to end as stopped", || ended().is_some()).await;
        let resident = state.providers.resident_info(Source::Acp, &id).expect("its agent process");
        assert_eq!(resident.phase, droppen_sessions::ResidentPhase::Idle, "and then the session is idle");
    }

    /// A message whose `session/prompt` cannot reach the agent once its model
    /// was switched fails, and says so, as one whose switch the agent never
    /// answers does: an error in the transcript, its receipt failed, and the
    /// session's last turn failed. Here the agent stops reading once it has
    /// switched, and the message is longer than a pipe holds, so its prompt is
    /// never written, and the error says the agent stopped reading.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_message_whose_prompt_cannot_be_written_after_its_switch_fails() {
        use droppen_provider::receipts::{now_ms, Claim, Kind};
        let home = tempfile::tempdir().expect("tempdir");
        let (state, id, _dir) = one_turn(
            home.path(),
            "FAKE_AGENT_DEAF=session/set_config_option=600",
            "hello",
            &SendOptions::default(),
        )
        .await;
        let injections = acp_of(&state);
        let receipts = injections.host.receipts().clone();
        let Claim::Owned { qid } = receipts
            .claim("client-1", Kind::Message, Source::Acp, Some(&id), now_ms())
            .expect("claimed")
        else {
            panic!("a new claim");
        };
        receipts.complete("client-1", "{}", None, now_ms());
        let options = SendOptions {
            model: Some("openrouter:vendor/big".into()),
            message_id: Some(qid),
            ..SendOptions::default()
        };
        injections
            .acp_send(state.providers.gate(), &id, &"x".repeat(1 << 20), &options)
            .await
            .expect("taken");
        until("the agent process to go", || {
            injections.acp_handle(&id).is_some_and(|handle| !handle.alive())
        })
        .await;

        let receipt = receipts
            .for_session(Source::Acp, &id, now_ms())
            .into_iter()
            .find(|receipt| receipt.client_message_id == "client-1")
            .expect("its receipt");
        assert_eq!(receipt.status, droppen_sessions::delivery::ReceiptStatus::Failed, "the message failed");
        let lines = transcript_lines(&state.paths, &id).await;
        assert_eq!(
            lines.last().map(String::as_str),
            Some(r#"{"message":"the agent did not read its input within 5s","type":"error","timestamp":"<timestamp>"}"#),
            "the transcript says why: the agent is there, and stopped reading"
        );
        let injection = injections
            .acp_handle(&id)
            .and_then(|handle| handle.injection())
            .expect("the session's last turn");
        assert_eq!(
            injection.state,
            droppen_sessions::InjectionRunState::Failed,
            "and so does the session's row"
        );
    }

    /// Start a session on the stand-in, configured with `env`, with one
    /// message, and wait for its turn to end; its id and working directory.
    async fn one_turn(home: &Path, env: &str, text: &str, options: &SendOptions) -> (AcpAlone, String, PathBuf) {
        let state = acp_alone(home);
        let injections = acp_of(&state);
        let gate = state.providers.gate();
        configure_fake(&state.paths, home, "fake", env);
        let dir = workdirs(home, &["work"]).remove(0);
        let id = injections.acp_start(gate, "fake", &dir.to_string_lossy(), text, options).await.expect("started");
        until("the turn", || injections.acp_handle(&id).is_some_and(|handle| !handle.busy())).await;
        (state, id, dir)
    }

    /// A session's first turn: its header, the message, the model and effort
    /// it runs on, then the turn as it streamed, each run of deltas followed
    /// by the record that closes it.
    #[tokio::test]
    async fn a_turn_is_written_down_line_for_line() {
        let home = tempfile::tempdir().expect("tempdir");
        let (state, id, dir) = one_turn(home.path(), "", "hi there", &SendOptions::default()).await;
        let lines = transcript_lines(&state.paths, &id).await;
        let cwd = json!(dir.to_string_lossy()).to_string();
        assert_eq!(
            lines[0],
            format!(r#"{{"id":"{id}","agent":"fake","agentSessionId":"f00dfeed0123456789abcdef01234567","cwd":{cwd},"type":"session","timestamp":"<timestamp>"}}"#)
        );
        assert_eq!(
            lines[1..],
            [
                r#"{"text":"hi there","images":[],"files":[],"type":"user","timestamp":"<timestamp>"}"#,
                r#"{"model":"fake:small","effort":"auto","type":"config","timestamp":"<timestamp>"}"#,
                r#"{"kind":"thinking","text":"Thinking about ","type":"delta","timestamp":"<timestamp>"}"#,
                r#"{"kind":"thinking","text":"hi there","type":"delta","timestamp":"<timestamp>"}"#,
                r#"{"text":"Thinking about hi there","type":"thinking","timestamp":"<timestamp>"}"#,
                r#"{"id":"call-1","title":"echo hi","kind":"execute","input":{"cmd":"echo hi"},"type":"tool_call","timestamp":"<timestamp>"}"#,
                r#"{"id":"call-1","status":"completed","output":"hi","type":"tool_result","timestamp":"<timestamp>"}"#,
                r#"{"kind":"text","text":"Hello ","type":"delta","timestamp":"<timestamp>"}"#,
                r#"{"kind":"text","text":"from the agent","type":"delta","timestamp":"<timestamp>"}"#,
                r#"{"used":4020,"size":128000,"type":"usage","timestamp":"<timestamp>"}"#,
                r#"{"text":"Hello from the agent","type":"assistant","timestamp":"<timestamp>"}"#,
                r#"{"stopReason":"end_turn","type":"turn_end","timestamp":"<timestamp>"}"#,
            ]
        );
    }

    /// A message's attachments are written down with it, and a model the
    /// model list would leave out is written down as `null`, not left out.
    #[tokio::test]
    async fn attachments_and_a_hidden_model_are_written_down_as_they_are() {
        let home = tempfile::tempdir().expect("tempdir");
        let options = SendOptions {
            images: vec!["/u/a.png".into()],
            files: vec!["/u/notes.txt".into(), "/u/b c.md".into()],
            ..SendOptions::default()
        };
        let (state, id, _) = one_turn(home.path(), "FAKE_AGENT_FIRST_MODEL=sk-ant-model-like-a-key", "see these", &options).await;
        assert_eq!(
            transcript_lines(&state.paths, &id).await[1..3],
            [
                r#"{"text":"see these","images":["/u/a.png"],"files":["/u/notes.txt","/u/b c.md"],"type":"user","timestamp":"<timestamp>"}"#,
                r#"{"model":null,"effort":"auto","type":"config","timestamp":"<timestamp>"}"#,
            ]
        );
    }

    /// A model the agent does not have fails the message before its turn: the
    /// message is written down, then why, and nothing of a turn.
    #[tokio::test]
    async fn a_model_the_agent_does_not_have_is_written_down_as_an_error() {
        let home = tempfile::tempdir().expect("tempdir");
        let picked = SendOptions {
            model: Some("not-a-model".into()),
            ..SendOptions::default()
        };
        let (state, id, _) = one_turn(home.path(), "", "hi", &picked).await;
        assert_eq!(
            transcript_lines(&state.paths, &id).await[1..],
            [
                r#"{"text":"hi","images":[],"files":[],"type":"user","timestamp":"<timestamp>"}"#,
                r#"{"message":"fake has no model \"not-a-model\"","type":"error","timestamp":"<timestamp>"}"#,
            ]
        );
    }

    /// A turn the agent answers with an error ends with what it streamed,
    /// closed, then the error in place of a `turn_end`.
    #[tokio::test]
    async fn a_turn_the_agent_fails_is_written_down_as_an_error() {
        let home = tempfile::tempdir().expect("tempdir");
        let (state, id, _) = one_turn(home.path(), "", "fail", &SendOptions::default()).await;
        assert_eq!(
            transcript_lines(&state.paths, &id).await[1..],
            [
                r#"{"text":"fail","images":[],"files":[],"type":"user","timestamp":"<timestamp>"}"#,
                r#"{"model":"fake:small","effort":"auto","type":"config","timestamp":"<timestamp>"}"#,
                r#"{"kind":"text","text":"Trying","type":"delta","timestamp":"<timestamp>"}"#,
                r#"{"text":"Trying","type":"assistant","timestamp":"<timestamp>"}"#,
                r#"{"message":"the model is unavailable","type":"error","timestamp":"<timestamp>"}"#,
            ]
        );
    }

    #[tokio::test]
    async fn a_check_that_gets_no_answer_leaves_nothing_running_and_no_scratch_directory() {
        let home = tempfile::tempdir().expect("tempdir");
        let state = acp_alone(home.path());
        let record = tempfile::tempdir().expect("tempdir");
        let agent = deaf_agent(record.path());
        let check = acp_of(&state).check_acp_agent_within(&agent, Duration::from_secs(2)).await;
        assert_eq!(check.state, AcpCheckState::Failed);
        assert!(
            check.error.0.as_deref().is_some_and(|error| error.contains("did not answer initialize")),
            "{check:?}"
        );
        let (scratch, helper) = recorded(record.path());
        assert!(
            scratch.file_name().is_some_and(|name| name.to_string_lossy().starts_with("droppen-acp-check-")),
            "{}",
            scratch.display()
        );
        assert!(!scratch.exists(), "{} is left behind", scratch.display());
        gone(helper).await;
    }

    #[tokio::test]
    async fn a_check_whose_request_is_cancelled_leaves_nothing_running_and_no_scratch_directory() {
        let home = tempfile::tempdir().expect("tempdir");
        let state = acp_alone(home.path());
        let record = tempfile::tempdir().expect("tempdir");
        let agent = deaf_agent(record.path());
        let cancelled = tokio::time::timeout(Duration::from_secs(2), acp_of(&state).check_acp_agent(&agent)).await;
        assert!(cancelled.is_err(), "still waiting for initialize when dropped");
        let (scratch, helper) = recorded(record.path());
        assert!(!scratch.exists(), "{} is left behind", scratch.display());
        gone(helper).await;
    }
    /// A step of a spawn, as [`droppen_provider::testing::Step`] records it.
    fn step(what: &'static str, gate: bool, claim: Option<bool>) -> droppen_provider::testing::Step {
        droppen_provider::testing::Step { what, gate, claim }
    }

    /// An ACP start holds the spawn gate only to put the session's placeholder
    /// in the map, and starts its agent outside it; a send that reopens a
    /// session does the same. An archive asks for the stop under the gate and
    /// waits for it outside. No ACP session is claimed.
    #[tokio::test]
    async fn acp_starts_and_archives_hold_the_gate_only_around_the_map() {
        let home = tempfile::tempdir().expect("tempdir");
        let state = acp_alone(home.path());
        let injections = acp_of(&state);
        let gate = state.providers.gate();
        configure_fake(&state.paths, home.path(), "fake", "");
        let dir = workdirs(home.path(), &["work"]).remove(0);
        let plain = SendOptions::default();

        let id = injections.acp_start(gate, "fake", &dir.to_string_lossy(), "hello", &plain).await.expect("started");
        assert_eq!(
            injections.host.steps(),
            [step("agent placeholder", true, Some(false)), step("agent started", false, Some(false))]
        );
        until("the first turn", || injections.acp_handle(&id).is_some_and(|handle| !handle.busy())).await;

        if let Some(stopping) = injections.acp_handle(&id).expect("a handle").request_stop() {
            let _ = stopping.await;
        }
        until("the process to go", || injections.acp_handle(&id).is_none_or(|handle| !handle.alive())).await;
        injections.acp_send(gate, &id, "again", &plain).await.expect("reopened");
        assert_eq!(
            injections.host.steps()[2..],
            [step("agent placeholder", true, Some(false)), step("agent started", false, Some(false))]
        );
        until("the second turn", || injections.acp_handle(&id).is_some_and(|handle| !handle.busy())).await;

        tokio::time::timeout(Duration::from_secs(10), injections.archive_acp(gate, &id))
            .await
            .expect("the archive waits for its agent's stop outside the gate")
            .expect("archived");
        assert_eq!(injections.host.steps()[4..], [step("agent stopping", false, Some(false))]);
    }

    /// The provider registry lends its own spawn gate, the one its routes take
    /// too: a start and a send through it hold that gate around the map, and
    /// an archive through it waits for as long as that gate is held.
    #[tokio::test]
    async fn the_provider_registry_lends_its_own_gate() {
        let home = tempfile::tempdir().expect("tempdir");
        let state = acp_alone(home.path());
        let (providers, injections) = (state.providers.clone(), acp_of(&state));
        configure_fake(&state.paths, home.path(), "fake", "");
        let dir = workdirs(home.path(), &["work"]).remove(0).to_string_lossy().into_owned();
        let options = SendOptions {
            acp_agent: Some("fake".into()),
            ..SendOptions::default()
        };

        let id = providers.start_new_session(Source::Acp, &dir, "hello", &options).await.expect("started");
        assert_eq!(injections.host.steps()[0], step("agent placeholder", true, Some(false)));
        until("the first turn", || injections.acp_handle(&id).is_some_and(|handle| !handle.busy())).await;
        if let Some(stopping) = injections.acp_handle(&id).expect("a handle").request_stop() {
            let _ = stopping.await;
        }
        until("the process to go", || injections.acp_handle(&id).is_none_or(|handle| !handle.alive())).await;
        providers.send_to_session(Source::Acp, &id, None, "again", &SendOptions::default()).await.expect("reopened");
        assert_eq!(injections.host.steps()[2], step("agent placeholder", true, Some(false)));
        until("the second turn", || injections.acp_handle(&id).is_some_and(|handle| !handle.busy())).await;

        let held = providers.gate().permit().await;
        let archive = tokio::spawn({
            let (providers, id) = (providers.clone(), id.clone());
            async move { providers.archive(Source::Acp, &id).await }
        });
        // Among the registry's gate's waiters: an archive lent another gate
        // never would be.
        until("the archive to wait for the registry's gate", || providers.gate().waiting() == 1).await;
        drop(held);
        tokio::time::timeout(Duration::from_secs(10), archive)
            .await
            .expect("the archive ends once the registry's gate is free: it waits for its agent's stop outside it")
            .expect("joined")
            .expect("archived");
    }
    /// Asking an agent what it offers, or checking that it answers, starts it
    /// in a scratch directory and outside the spawn gate: neither runs a
    /// session, and a slow agent must not hold up every send.
    #[tokio::test]
    async fn a_probe_and_a_check_start_their_agent_outside_the_gate() {
        let home = tempfile::tempdir().expect("tempdir");
        let state = acp_alone(home.path());
        let injections = acp_of(&state);
        configure_fake(&state.paths, home.path(), "fake", "");
        let agent = AcpAgentRegistry::of(&state.paths).get("fake").expect("configured");

        injections.acp_options(&agent).await.expect("its options");
        let check = injections.check_acp_agent(&agent).await;
        assert_eq!(check.state, AcpCheckState::Ready, "{check:?}");
        assert_eq!(
            injections.host.steps(),
            [step("agent probed", false, None), step("agent checked", false, None)]
        );
    }
}
