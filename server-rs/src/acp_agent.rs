//! The ACP agents this machine runs sessions on: a registry keyed by id.
//!
//! Any program that speaks the [Agent Client Protocol](https://agentclientprotocol.com)
//! over stdio can run Droppen sessions ([`crate::inject::acp`]). An agent is a
//! description — an id, the name it goes by, the command line that starts it in
//! ACP mode — plus the environment the user gives it. What it can do is not part
//! of the description: that is what it answers to `initialize` and publishes as
//! its sessions' configuration options.
//!
//! * **Built-in** descriptions are `acp-agents.json`, embedded in the build:
//!   the agents Droppen knows how to start ([`presets`]). They are offered when
//!   an agent is added and never run on their own. An agent is often installed
//!   into an environment of its own that the server's `PATH` knows nothing
//!   about, and its model settings often come from variables a shell profile or
//!   a project `.env` sets, so only the user can say how it runs here.
//! * **Configured** agents are rows in the machine-wide state database under
//!   [`scope::ACP_AGENTS`], one per id, holding `name`, `command`, `args` and
//!   `env` as JSON. One with a built-in's id is that agent as the user set it
//!   up (a name left empty is the built-in's); any other id is the user's own.
//!   These are the agents sessions run on ([`Paths::acp_agents`],
//!   [`Paths::acp_agent`]).
//!
//! The id is a short slug ([`valid_id`]): it names the directory an agent's
//! transcripts are kept in ([`crate::sessions::acp_transcript`]) and is
//! recorded on every session, so a session is reopened by the agent it ran on
//! and no other. Removing an agent leaves its sessions alone: they stay listed
//! and readable, and cannot be continued until an agent with that id is
//! configured again. It is refused while a turn of the agent is running or a
//! message is waiting for one.
//!
//! A **check** ([`AcpCheck`], `POST /api/acp/agents/{id}/check`) starts the
//! agent with its environment, sends `initialize`, reads what it says it is and
//! can do, and stops it; nothing reaches a model. The last one is kept with the
//! agent and forgotten when its command line or environment changes, so the
//! settings page and the new-session picker can say an agent cannot start
//! before a session finds out.
//!
//! `env` is read through [`parse_env_block`], so an agent's block refuses what
//! an account's block refuses: `PATH`, the loader variables, and everything
//! else that would move where a child's state lives. The values are what an
//! API key looks like, so they are handled the way [`crate::chats::provider`]
//! handles its key: a client is told the names, never the values, and a write
//! replaces the whole block or leaves it alone.

use std::path::PathBuf;
use std::sync::LazyLock;

use gotcha::{api, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use crate::accounts::store::{parse_env_block, EnvVar};
use crate::api::error::ApiError;
use crate::api::extract::{ApiJson, ApiPath};
use crate::api::{ApiRouter, AppState, SendResult};
use crate::db::scope;
use crate::openapi_ext::{ExtraResponses, Optional};
use crate::process_env::{clean_env_bare, resolve_on_path, Env};
use crate::sessions::{Paths, SessionSummary, Source};

/// The longest agent id.
const MAX_ID: usize = 64;
/// The longest name a person needs.
const MAX_NAME: usize = 64;
/// The longest executable path a person can mean.
const MAX_COMMAND: usize = 4096;
/// The most arguments, and the longest one.
const MAX_ARGS: usize = 64;
const MAX_ARG: usize = 4096;
/// The most agents one machine configures.
const MAX_AGENTS: usize = 64;

/// Whether `id` can name an agent: ASCII lowercase letters, digits and `-`,
/// starting with a letter or digit. It is a directory name and a key, so
/// nothing else is let near either.
pub fn valid_id(id: &str) -> bool {
    id.len() <= MAX_ID
        && id.bytes().next().is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// One built-in agent description: how a known agent is started in ACP mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AcpAgentPreset {
    /// Its id, which a configured agent made from it keeps.
    pub id: String,
    /// What the agent is called.
    pub name: String,
    /// The executable, as found on the `PATH`.
    pub command: String,
    /// The arguments that put it into ACP mode over stdio.
    pub args: Vec<String>,
    /// Where its ACP setup is documented.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub homepage: Optional<String>,
    /// What it needs before it can run.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub note: Optional<String>,
    /// Whether its executable is on this server's `PATH`, as a lookup finds it
    /// (nothing is started). Filled in when the descriptions are listed.
    #[serde(default)]
    pub on_path: bool,
}

impl AcpAgentPreset {
    /// This description, with whether its executable is on `PATH` in `env`.
    fn looked_up(&self, env: &Env) -> Self {
        Self {
            on_path: resolve_on_path(&self.command, env).is_some(),
            ..self.clone()
        }
    }
}

static PRESETS: LazyLock<Vec<AcpAgentPreset>> =
    LazyLock::new(|| serde_json::from_str(include_str!("acp-agents.json")).expect("acp-agents.json is a list of agent descriptions"));

/// Every built-in description, in the order they are offered.
pub fn presets() -> &'static [AcpAgentPreset] {
    &PRESETS
}

fn preset(id: &str) -> Option<&'static AcpAgentPreset> {
    presets().iter().find(|preset| preset.id == id)
}

/// One configured agent, values included. Never serialized to a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpAgent {
    /// Its id ([`valid_id`]).
    pub id: String,
    /// What the user calls it, in every message about it.
    pub name: String,
    /// The executable, absolute or looked up on the `PATH` in its environment.
    pub command: String,
    /// Its arguments: whatever puts it into ACP mode over stdio.
    pub args: Vec<String>,
    /// Extra environment, in the order it was typed.
    pub env: Vec<EnvVar>,
    /// What the last check of this command line and environment found.
    pub last_check: Option<AcpCheck>,
}

impl AcpAgent {
    /// The executable to run.
    pub fn program(&self) -> PathBuf {
        PathBuf::from(self.command.trim())
    }

    /// Whether `other` starts the same process: the same command line and the
    /// same environment.
    pub fn launches_like(&self, other: &AcpAgent) -> bool {
        self.command == other.command && self.args == other.args && self.env == other.env
    }

    /// `base` with the configured variables laid over it.
    pub fn environment(&self, mut base: Env) -> Env {
        for var in &self.env {
            base.insert(var.key.clone(), var.value.clone());
        }
        base
    }

    /// What a client may learn.
    pub fn info(&self) -> AcpAgentInfo {
        let preset = preset(&self.id);
        AcpAgentInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            command: self.command.clone(),
            args: self.args.clone(),
            env_names: self.env.iter().map(|var| var.key.clone()).collect(),
            homepage: preset.and_then(|preset| preset.homepage.0.clone()).into(),
            note: preset.and_then(|preset| preset.note.0.clone()).into(),
            last_check: self.last_check.clone().map(AcpCheck::without_error).into(),
        }
    }

    /// `text` with this agent's variables' values taken out
    /// ([`crate::process_env::redact_values`]): they are what an API key looks
    /// like, and anything it says may repeat one.
    pub fn without_values(&self, text: &str) -> String {
        crate::process_env::redact_values(text, self.env.iter().map(|var| var.value.as_str()))
    }

    /// Keep `check` as this agent's last, unless its command line or
    /// environment changed while it ran: then it describes nothing that is
    /// configured, and is dropped. `false` when it was not kept.
    pub fn record_check(&self, paths: &Paths, check: &AcpCheck) -> bool {
        let _writing = WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let db = crate::db::at(paths.config_db());
        let Some(mut stored) = db.kv_get(scope::ACP_AGENTS, &self.id).and_then(|raw| serde_json::from_str::<Stored>(&raw).ok()) else {
            return false;
        };
        if !stored.launches_like(self) {
            return false;
        }
        stored.last_check = Some(check.clone());
        match serde_json::to_string(&stored) {
            Ok(raw) => {
                db.kv_set(scope::ACP_AGENTS, &self.id, &raw);
                true
            }
            Err(_) => false,
        }
    }
}

/// A row of [`scope::ACP_AGENTS`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    name: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: Vec<EnvVar>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_check: Option<AcpCheck>,
}

impl Stored {
    /// Whether this row starts the same process `agent` does: the same command
    /// line and the same environment.
    fn launches_like(&self, agent: &AcpAgent) -> bool {
        self.command == agent.command && self.args == agent.args && self.env == agent.env
    }

    /// What a new agent with this id starts from: its built-in's description, if
    /// it has one.
    fn fresh(id: &str) -> Self {
        preset(id).map_or_else(Stored::default, |preset| Stored {
            name: preset.name.clone(),
            command: preset.command.clone(),
            args: preset.args.clone(),
            env: Vec::new(),
            last_check: None,
        })
    }

    fn agent(self, id: &str) -> AcpAgent {
        let name = match self.name.trim() {
            "" => preset(id).map_or_else(|| id.to_string(), |preset| preset.name.clone()),
            name => name.to_string(),
        };
        AcpAgent {
            id: id.to_string(),
            name,
            command: self.command,
            args: self.args,
            env: self.env,
            last_check: self.last_check,
        }
    }
}

/// A stored row, or `None` for one this build cannot read or that names no
/// executable: an agent that cannot be started is no agent.
fn parse(id: &str, raw: &str) -> Option<AcpAgent> {
    let stored: Stored = serde_json::from_str(raw).ok()?;
    (!stored.command.trim().is_empty()).then(|| stored.agent(id))
}

/// What a message to a session of the removed agent `id` is told.
pub fn removed(id: &str) -> String {
    format!("the ACP agent \"{id}\" is no longer configured; add it again in Settings to continue this session")
}

/// One write at a time: an update reads the row it replaces.
static WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The configured agents, kept in the machine-wide state database.
impl Paths {
    /// Every configured agent, in id order.
    pub fn acp_agents(&self) -> Vec<AcpAgent> {
        crate::db::at(self.config_db())
            .kv_scope(scope::ACP_AGENTS)
            .into_iter()
            .filter(|(id, _)| valid_id(id))
            .filter_map(|(id, raw)| parse(&id, &raw))
            .collect()
    }

    /// The configured agent `id`, or `None` when there is none.
    pub fn acp_agent(&self, id: &str) -> Option<AcpAgent> {
        if !valid_id(id) {
            return None;
        }
        let raw = crate::db::at(self.config_db()).kv_get(scope::ACP_AGENTS, id)?;
        parse(id, &raw)
    }

    /// Why no message can go to this session, when the reason is its agent: an ACP
    /// session whose agent is no longer configured. `None` for every other
    /// session.
    ///
    /// The session itself stays as it was — listed, readable, archivable — and an
    /// agent configured again under the same id picks it up where it stopped.
    pub fn acp_send_block(&self, summary: &SessionSummary) -> Option<String> {
        if summary.source != Source::Acp {
            return None;
        }
        match summary.acp_agent.as_deref() {
            Some(id) if self.acp_agent(id).is_some() => None,
            Some(id) => Some(removed(id)),
            None => Some("this ACP session does not say which agent it ran on".into()),
        }
    }

    /// Apply a patch to the agent `id`, adding it when there is none, and persist
    /// it; or say why it was refused. A new agent with a built-in's id starts from
    /// that description.
    pub fn update_acp_agent(&self, id: &str, patch: &AcpAgentUpdate) -> Result<AcpAgent, String> {
        if !valid_id(id) {
            return Err(format!(
                "an agent id is at most {MAX_ID} lowercase letters, digits and dashes, starting with a letter or digit"
            ));
        }
        let _writing = WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let db = crate::db::at(self.config_db());
        let rows = db.kv_scope(scope::ACP_AGENTS);
        let mut next = match rows.get(id).and_then(|raw| serde_json::from_str::<Stored>(raw).ok()) {
            Some(stored) => stored,
            None if rows.len() >= MAX_AGENTS => return Err(format!("at most {MAX_AGENTS} ACP agents can be configured")),
            None => Stored::fresh(id),
        };
        let launch = (next.command.clone(), next.args.clone(), next.env.clone());
        if let Some(name) = patch.name.as_deref() {
            let name = name.trim();
            if name.len() > MAX_NAME || name.chars().any(char::is_control) {
                return Err(format!("the agent's name must be a single line of at most {MAX_NAME} bytes"));
            }
            next.name = name.to_string();
        }
        if let Some(command) = patch.command.as_deref() {
            let command = command.trim();
            if command.len() > MAX_COMMAND || command.contains(['\n', '\r', '\0']) {
                return Err("the agent's executable must be a single path or command name".into());
            }
            // A relative path would be found from whatever directory the agent is
            // started in, which is the session's.
            if command.contains('/') && !command.starts_with('/') {
                return Err("the agent's executable must be an absolute path, or a command name to look up on its PATH".into());
            }
            next.command = command.to_string();
        }
        if let Some(args) = patch.args.0.as_deref() {
            if args.len() > MAX_ARGS || args.iter().any(|arg| arg.len() > MAX_ARG || arg.contains(['\n', '\r', '\0'])) {
                return Err(format!("the agent's arguments must be at most {MAX_ARGS} single-line words"));
            }
            next.args = args.to_vec();
        }
        if let Some(block) = patch.env.as_deref() {
            next.env = parse_block(block)?;
        }
        if next.command.is_empty() {
            return Err("an agent needs an executable".into());
        }
        // Another command line or environment is another process: what the last
        // check found says nothing about it.
        if (&next.command, &next.args, &next.env) != (&launch.0, &launch.1, &launch.2) {
            next.last_check = None;
        }
        let raw = serde_json::to_string(&next).map_err(|error| error.to_string())?;
        db.kv_set(scope::ACP_AGENTS, id, &raw);
        Ok(next.agent(id))
    }

    /// Forget the agent `id`; `false` when there was none. Its sessions stay.
    pub fn remove_acp_agent(&self, id: &str) -> bool {
        if !valid_id(id) {
            return false;
        }
        let _writing = WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let db = crate::db::at(self.config_db());
        if db.kv_get(scope::ACP_AGENTS, id).is_none() {
            return false;
        }
        db.kv_remove(scope::ACP_AGENTS, id);
        true
    }
}

/// [`parse_env_block`], except that a block with nothing in it clears the
/// variables rather than being refused: an account needs a credential, an
/// agent may not.
fn parse_block(block: &str) -> Result<Vec<EnvVar>, String> {
    let empty = block
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .all(|line| line.is_empty() || line.starts_with('#'));
    if empty {
        return Ok(Vec::new());
    }
    parse_env_block(block).map_err(|error| error.to_string())
}

/// One configured agent, as a client sees it: the variables' names, not their
/// values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AcpAgentInfo {
    /// Its id: what sessions record, and what `presets/icons/<id>.svg` draws.
    pub id: String,
    /// What the user calls it.
    pub name: String,
    /// The configured executable.
    pub command: String,
    /// Its arguments, in order.
    pub args: Vec<String>,
    /// The names of the extra environment variables, in order.
    pub env_names: Vec<String>,
    /// Where its ACP setup is documented, for a built-in agent.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub homepage: Optional<String>,
    /// What a built-in agent needs before it can run.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub note: Optional<String>,
    /// What the last check of its command line and environment found, without
    /// its `error` (`GET /api/acp/agents/{id}/check` has it); absent until one
    /// ran.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub last_check: Optional<AcpCheck>,
}

/// How a check of an agent ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "snake_case")]
pub enum AcpCheckState {
    /// It started and answered `initialize`.
    Ready,
    /// Its executable is not there: not on its `PATH`, or not an executable file.
    NotFound,
    /// It did not start, did not answer `initialize` in time, or answered it
    /// with an error.
    Failed,
}

/// What a check found (`POST /api/acp/agents/{id}/check`): the agent was
/// started with its environment, sent `initialize` and stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AcpCheck {
    /// When, ISO-8601.
    pub checked_at: String,
    /// How it ended.
    pub state: AcpCheckState,
    /// Where its executable was found.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub resolved_path: Optional<String>,
    /// The ACP version it agreed to.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub protocol_version: Optional<u64>,
    /// What it says it is.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub agent_info: Optional<AcpAgentIdentity>,
    /// What it says it can do.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub capabilities: Optional<AcpCapabilities>,
    /// The ways it can be signed in to, when it needs that first.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub auth_methods: Optional<Vec<AcpAuthMethod>>,
    /// Why it is not ready: the error, the end of what it wrote to stderr and
    /// the `PATH` it was looked up on, with the agent's variable values and
    /// anything that looks like a key taken out. Never in a list of the agents,
    /// which every device may read: only in a check's own answer.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub error: Optional<String>,
}

/// An agent's `agentInfo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AcpAgentIdentity {
    /// Its programmatic name.
    pub name: String,
    /// Its display name.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub title: Optional<String>,
    /// Its version.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub version: Optional<String>,
}

/// What an agent says it can do, as `initialize` answers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AcpCapabilities {
    /// It can reopen a session (`session/load`).
    pub load_session: bool,
    /// A prompt may hold images.
    pub image: bool,
    /// A prompt may hold audio.
    pub audio: bool,
    /// A prompt may embed resources.
    pub embedded_context: bool,
    /// It can be given MCP servers over HTTP.
    pub mcp_http: bool,
    /// It can be given MCP servers over SSE.
    pub mcp_sse: bool,
}

/// One way an agent can be signed in to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AcpAuthMethod {
    /// Its id.
    pub id: String,
    /// What it is called.
    pub name: String,
    /// What it does.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub description: Optional<String>,
}

/// The longest error a check keeps, and what an agent says in a session's
/// error is cut to ([`crate::inject::InjectionRegistry::redact_acp`]).
pub(crate) const MAX_CHECK_ERROR: usize = 4096;
/// The longest name, title, version or id of what an agent says it is, and
/// of a model or effort it offers, in characters; and the longest description
/// of a way to sign in.
pub(crate) const MAX_AGENT_WORD: usize = 256;
const MAX_CHECK_DESCRIPTION: usize = 1024;
/// The most ways to sign in a check keeps.
const MAX_AUTH_METHODS: usize = 16;

impl AcpCheck {
    /// An agent that started and answered `initialize` with `answer`. What it
    /// says of itself goes through `redact`, as its errors do, and is cut short
    /// where it is longer than any name: it is kept, and shown to whoever
    /// lists the agents.
    pub fn ready(resolved: PathBuf, answer: &crate::acp::InitializeResponse, redact: impl Fn(&str) -> String) -> Self {
        let caps = &answer.agent_capabilities;
        let said = |text: &str, cap: usize| crate::accounts::limits::clip(&redact(text), cap);
        let word = |text: &str| said(text, MAX_AGENT_WORD);
        Self {
            resolved_path: Some(resolved.to_string_lossy().into_owned()).into(),
            protocol_version: answer.protocol_version.into(),
            agent_info: answer
                .agent_info
                .as_ref()
                .map(|info| AcpAgentIdentity {
                    name: word(&info.name),
                    title: info.title.as_deref().map(word).into(),
                    version: info.version.as_deref().map(word).into(),
                })
                .into(),
            capabilities: Some(AcpCapabilities {
                load_session: caps.load_session,
                image: caps.prompt_capabilities.image,
                audio: caps.prompt_capabilities.audio,
                embedded_context: caps.prompt_capabilities.embedded_context,
                mcp_http: caps.mcp_capabilities.http,
                mcp_sse: caps.mcp_capabilities.sse,
            })
            .into(),
            auth_methods: (!answer.auth_methods.is_empty())
                .then(|| {
                    answer
                        .auth_methods
                        .iter()
                        .take(MAX_AUTH_METHODS)
                        .map(|method| AcpAuthMethod {
                            id: word(&method.id),
                            name: word(&method.name),
                            description: method.description.as_deref().map(|text| said(text, MAX_CHECK_DESCRIPTION)).into(),
                        })
                        .collect()
                })
                .into(),
            ..Self::ended(AcpCheckState::Ready, None)
        }
    }

    /// A check that ended in `state`, with `error` saying why. The error is
    /// cut to [`MAX_CHECK_ERROR`] bytes; redacting it is the caller's.
    pub fn ended(state: AcpCheckState, error: Option<String>) -> Self {
        let error = error.map(|mut error| {
            if error.len() > MAX_CHECK_ERROR {
                let mut end = MAX_CHECK_ERROR;
                while !error.is_char_boundary(end) {
                    end -= 1;
                }
                error.truncate(end);
                error.push('\u{2026}');
            }
            error
        });
        Self {
            checked_at: crate::sessions::iso8601(std::time::SystemTime::now()),
            state,
            resolved_path: None.into(),
            protocol_version: None.into(),
            agent_info: None.into(),
            capabilities: None.into(),
            auth_methods: None.into(),
            error: error.into(),
        }
    }

    /// Whether the agent cannot run, as far as this check knows.
    pub fn failed(&self) -> bool {
        self.state != AcpCheckState::Ready
    }

    /// This, without what the agent said: how it ended and what it is, as an
    /// agent's listing reports it.
    fn without_error(self) -> Self {
        Self {
            error: None.into(),
            ..self
        }
    }
}

/// `GET /api/acp/agents`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct AcpAgentList {
    /// The agents sessions can run on, in id order.
    pub agents: Vec<AcpAgentInfo>,
    /// The built-in descriptions, to offer when an agent is added, each with
    /// whether its executable is on this server's `PATH`.
    pub presets: Vec<AcpAgentPreset>,
}

/// The body of `PUT /api/acp/agents/{id}`; every field is a patch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AcpAgentUpdate {
    /// The name; `""` goes back to the built-in's or the id, absent leaves it alone.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub name: Optional<String>,
    /// The executable; required unless the agent exists or is a built-in.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub command: Optional<String>,
    /// The arguments; absent leaves them alone.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub args: Optional<Vec<String>>,
    /// A `KEY=VALUE`-per-line block that replaces every stored variable; `""`
    /// clears them, absent leaves them alone.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub env: Optional<String>,
}

/// The ACP agents configured on this machine, and the built-in descriptions.
#[api(id = "acp_agents", group = "acp", summary = "ACP agents")]
pub async fn acp_agents(State(state): State<AppState>) -> Json<AcpAgentList> {
    let env = clean_env_bare();
    Json(AcpAgentList {
        agents: state.paths.acp_agents().iter().map(AcpAgent::info).collect(),
        presets: presets().iter().map(|preset| preset.looked_up(&env)).collect(),
    })
}

/// Add an ACP agent or change how it is started. A changed command line or
/// environment renews the processes its sessions run in: one with nothing to
/// do ends now, one with a turn or messages waiting when they are done, and
/// each session's next message starts the agent as it is configured now.
#[api(id = "put_acp_agent", group = "acp", summary = "Add or update an ACP agent")]
pub async fn put_acp_agent(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<AcpAgentUpdate>,
) -> Result<Json<AcpAgentInfo>, ApiError> {
    let agent = state
        .drive
        .injections
        .reconfigure_acp_agent(&id, &body)
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(agent.info()))
}

/// Remove an ACP agent. Its sessions stay listed and readable; they cannot be
/// continued until an agent with this id is configured again, which starts
/// their processes afresh: the ones they had, idle by then, are stopped.
/// Refused while a turn of the agent is running, or a message is waiting for
/// one.
#[api(id = "delete_acp_agent", group = "acp", summary = "Remove an ACP agent")]
pub async fn delete_acp_agent(State(state): State<AppState>, ApiPath(id): ApiPath<String>) -> Result<Json<SendResult>, ApiError> {
    let Some(agent) = state.paths.acp_agent(&id) else {
        return Err(ApiError::not_found("no such ACP agent"));
    };
    match state.drive.injections.remove_acp_agent(&agent).await {
        Ok(true) => Ok(Json(SendResult { accepted: true })),
        Ok(false) => Err(ApiError::not_found("no such ACP agent")),
        Err(busy) => Err(ApiError::conflict(busy)),
    }
}

/// Check an ACP agent: start it with its environment, send `initialize`, read
/// what it says it is and can do, and stop it. Nothing is sent to a model. The
/// answer is kept as the agent's `lastCheck`.
#[api(id = "check_acp_agent", group = "acp", summary = "Check an ACP agent")]
pub async fn check_acp_agent(State(state): State<AppState>, ApiPath(id): ApiPath<String>) -> Result<Json<AcpCheck>, ApiError> {
    let agent = state.paths.acp_agent(&id).ok_or_else(|| ApiError::not_found("no such ACP agent"))?;
    let check = state.drive.injections.check_acp_agent(&agent).await;
    agent.record_check(&state.paths, &check);
    Ok(Json(check))
}

/// An ACP agent's last check in full: the error it ended with included, which
/// is the agent's own words, the end of its stderr among them. A route of its
/// own so that the list, which every device reads, never carries it.
#[api(id = "last_acp_check", group = "acp", summary = "The last check of an ACP agent")]
pub async fn last_acp_check(State(state): State<AppState>, ApiPath(id): ApiPath<String>) -> Result<Json<AcpCheck>, ApiError> {
    let agent = state.paths.acp_agent(&id).ok_or_else(|| ApiError::not_found("no such ACP agent"))?;
    agent
        .last_check
        .map(Json)
        .ok_or_else(|| ApiError::not_found("this ACP agent has not been checked"))
}

/// Register this module's routes.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router
        .get("/api/acp/agents", acp_agents)
        .put("/api/acp/agents/{id}", put_acp_agent)
        .delete("/api/acp/agents/{id}", delete_acp_agent)
        .post("/api/acp/agents/{id}/check", check_acp_agent)
        .get("/api/acp/agents/{id}/check", last_acp_check)
}

/// Everything gotcha cannot express about those routes.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe(
        "GET",
        "/api/acp/agents",
        200,
        "The configured ACP agents, with the names of their extra variables, and the built-in descriptions",
    )
    .describe("PUT", "/api/acp/agents/{id}", 200, "The agent as now configured")
    .error(
        "PUT",
        "/api/acp/agents/{id}",
        400,
        "An unusable id, name, executable, argument or environment block",
    )
    .describe("DELETE", "/api/acp/agents/{id}", 200, "The agent is gone; its sessions stay")
    .error("DELETE", "/api/acp/agents/{id}", 404, "No such ACP agent")
    .error(
        "DELETE",
        "/api/acp/agents/{id}",
        409,
        "A turn of the agent is running, or a message is waiting for one",
    )
    .describe(
        "POST",
        "/api/acp/agents/{id}/check",
        200,
        "What the check found: ready, or why the agent cannot start",
    )
    .error("POST", "/api/acp/agents/{id}/check", 404, "No such ACP agent")
    .describe("GET", "/api/acp/agents/{id}/check", 200, "The agent's last check, its error included")
    .error(
        "GET",
        "/api/acp/agents/{id}/check",
        404,
        "No such ACP agent, or it has not been checked",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> (tempfile::TempDir, Paths) {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        (home, paths)
    }

    fn patch(command: Option<&str>, args: Option<&[&str]>, env: Option<&str>) -> AcpAgentUpdate {
        AcpAgentUpdate {
            name: None.into(),
            command: command.map(str::to_owned).into(),
            args: args.map(|args| args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>()).into(),
            env: env.map(str::to_owned).into(),
        }
    }

    #[test]
    fn the_built_in_descriptions_are_usable() {
        assert!(!presets().is_empty());
        let mut ids: Vec<&str> = presets().iter().map(|preset| preset.id.as_str()).collect();
        for preset in presets() {
            assert!(valid_id(&preset.id), "{}", preset.id);
            assert!(!preset.name.is_empty() && !preset.command.is_empty(), "{preset:?}");
        }
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), presets().len(), "ids are unique");
    }

    #[test]
    fn an_id_is_a_slug() {
        for good in ["bub", "open-code", "a1", "9", &"x".repeat(MAX_ID)] {
            assert!(valid_id(good), "{good}");
        }
        for bad in ["", "-x", "Bub", "a_b", "a.b", "../x", "a/b", "é", &"x".repeat(MAX_ID + 1)] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn nothing_stored_is_no_agent() {
        let (_home, paths) = paths();
        assert!(paths.acp_agents().is_empty());
        assert!(paths.acp_agent("bub").is_none(), "a built-in is offered, not run");
    }

    #[test]
    fn values_are_kept_and_never_reported() {
        let (_home, paths) = paths();
        paths
            .update_acp_agent(
                "mine",
                &AcpAgentUpdate {
                    name: Some(" My agent ".to_string()).into(),
                    ..patch(
                        Some(" /opt/agent/bin/agent "),
                        Some(&["acp", "--stdio"]),
                        Some("AGENT_MODEL=openai:gpt-5\nexport AGENT_API_KEY=\"sk-secret\""),
                    )
                },
            )
            .expect("saved");
        let stored = paths.acp_agent("mine").expect("configured");
        assert_eq!(stored.name, "My agent");
        assert_eq!(stored.program(), PathBuf::from("/opt/agent/bin/agent"));
        assert_eq!(stored.args, vec!["acp", "--stdio"]);
        let info = serde_json::to_string(&stored.info()).unwrap();
        assert!(info.contains("AGENT_API_KEY"), "{info}");
        assert!(!info.contains("sk-secret"), "a value never reaches a client: {info}");
        let env = stored.environment(Env::from([("HOME".to_string(), "/h".to_string())]));
        assert_eq!(env.get("AGENT_API_KEY").map(String::as_str), Some("sk-secret"));
        assert_eq!(env.get("HOME").map(String::as_str), Some("/h"));
    }

    #[test]
    fn two_agents_are_kept_apart() {
        let (_home, paths) = paths();
        paths.update_acp_agent("one", &patch(Some("/x/one"), Some(&["acp"]), Some("A=1"))).expect("saved");
        paths.update_acp_agent("two", &patch(Some("/x/two"), None, Some("B=2"))).expect("saved");
        let listed: Vec<(String, String)> = paths.acp_agents().into_iter().map(|agent| (agent.id, agent.command)).collect();
        assert_eq!(listed, vec![("one".into(), "/x/one".into()), ("two".into(), "/x/two".into())]);
        assert_eq!(paths.acp_agent("one").unwrap().info().env_names, vec!["A"]);
        assert_eq!(paths.acp_agent("two").unwrap().info().env_names, vec!["B"]);
        assert_eq!(paths.acp_agent("two").unwrap().name, "two", "a name left empty is the id");
        assert!(paths.remove_acp_agent("one"));
        assert!(!paths.remove_acp_agent("one"), "gone already");
        assert!(paths.acp_agent("one").is_none());
        assert!(paths.acp_agent("two").is_some(), "the other stays");
    }

    #[test]
    fn a_built_in_is_added_from_its_description() {
        let (_home, paths) = paths();
        let bub = presets().iter().find(|preset| preset.id == "bub").expect("bub is built in");
        let added = paths.update_acp_agent("bub", &patch(None, None, Some("BUB_MODEL=x"))).expect("saved");
        assert_eq!((added.name.as_str(), added.command.as_str()), (bub.name.as_str(), bub.command.as_str()));
        assert_eq!(added.args, bub.args);
        assert_eq!(added.info().homepage, bub.homepage);
        // Its own name and command line, once changed, are the user's.
        paths.update_acp_agent("bub", &patch(Some("/home/me/.venv/bin/bub"), None, None)).expect("saved");
        assert_eq!(paths.acp_agent("bub").unwrap().command, "/home/me/.venv/bin/bub");
        assert_eq!(paths.acp_agent("bub").unwrap().info().env_names, vec!["BUB_MODEL"]);
    }

    #[test]
    fn an_absent_field_is_left_alone_and_an_empty_one_clears() {
        let (_home, paths) = paths();
        paths.update_acp_agent("x", &patch(Some("/x/agent"), Some(&["acp"]), Some("A=1"))).expect("saved");
        paths.update_acp_agent("x", &patch(None, None, None)).expect("saved");
        let stored = paths.acp_agent("x").unwrap();
        assert_eq!((stored.command.as_str(), stored.args.clone(), stored.info().env_names), ("/x/agent", vec!["acp".to_string()], vec!["A".to_string()]));
        paths.update_acp_agent("x", &patch(None, Some(&[]), Some("  \n# nothing\n"))).expect("saved");
        let stored = paths.acp_agent("x").unwrap();
        assert!(stored.args.is_empty() && stored.env.is_empty());
    }

    #[test]
    fn what_cannot_run_or_would_move_the_child_is_refused() {
        let (_home, paths) = paths();
        assert!(paths.update_acp_agent("x", &patch(None, None, None)).is_err(), "a new agent needs an executable");
        assert!(paths.update_acp_agent("x", &patch(Some(" "), None, None)).is_err());
        assert!(paths.update_acp_agent("../x", &patch(Some("/x"), None, None)).is_err());
        assert!(paths.update_acp_agent("X", &patch(Some("/x"), None, None)).is_err());
        assert!(paths.update_acp_agent("x", &patch(Some("/x"), None, Some("PATH=/tmp"))).is_err());
        assert!(paths.update_acp_agent("x", &patch(Some("/x"), None, Some("not a pair"))).is_err());
        assert!(paths.update_acp_agent("x", &patch(Some("agent\nrm -rf /"), None, None)).is_err());
        assert!(paths.update_acp_agent("x", &patch(Some("/x"), Some(&["a\nb"]), None)).is_err());
        for relative in ["bin/agent", "./agent", "~/bin/agent"] {
            assert!(paths.update_acp_agent("x", &patch(Some(relative), None, None)).is_err(), "{relative}");
        }
        assert!(paths.acp_agents().is_empty(), "a refusal stores nothing");
        paths.update_acp_agent("y", &patch(Some("agent"), None, None)).expect("a command name is looked up on PATH");
        assert!(paths.remove_acp_agent("y"));
        paths.update_acp_agent("x", &patch(Some("/x"), None, None)).expect("saved");
        assert!(paths.update_acp_agent("x", &patch(Some(""), None, None)).is_err(), "removing is a delete, not an empty executable");
        assert_eq!(paths.acp_agent("x").unwrap().command, "/x");
    }

    #[test]
    fn a_check_is_kept_until_the_command_line_or_environment_changes() {
        let (_home, paths) = paths();
        paths.update_acp_agent("x", &patch(Some("/x/agent"), Some(&["acp"]), Some("KEY=a-long-secret"))).expect("saved");
        let configured = paths.acp_agent("x").unwrap();
        assert!(configured.last_check.is_none(), "nothing checked yet");
        let found = AcpCheck::ended(AcpCheckState::NotFound, Some("gone".into()));
        assert!(configured.record_check(&paths, &found));
        assert_eq!(agent_check(&paths, "x"), Some(AcpCheckState::NotFound));
        assert_eq!(paths.acp_agent("x").unwrap().last_check.and_then(|check| check.error.0), Some("gone".into()));
        assert_eq!(paths.acp_agent("x").unwrap().info().last_check.0.map(|check| check.error.0), Some(None), "a listing leaves it out");

        // A new name is the same process: the check stands.
        paths.update_acp_agent("x", &AcpAgentUpdate { name: Some("Renamed".into()).into(), ..AcpAgentUpdate::default() }).expect("saved");
        assert_eq!(agent_check(&paths, "x"), Some(AcpCheckState::NotFound));
        // Another environment is another process: it is forgotten.
        paths.update_acp_agent("x", &patch(None, None, Some("KEY=other"))).expect("saved");
        assert_eq!(agent_check(&paths, "x"), None);

        // A check of what was configured before an edit is not kept after it.
        let before = paths.acp_agent("x").unwrap();
        paths.update_acp_agent("x", &patch(Some("/y/agent"), None, None)).expect("saved");
        assert!(!before.record_check(&paths, &found), "it describes a command line that is gone");
        assert_eq!(agent_check(&paths, "x"), None);
    }

    fn agent_check(paths: &Paths, id: &str) -> Option<AcpCheckState> {
        paths.acp_agent(id).and_then(|agent| agent.last_check).map(|check| check.state)
    }

    #[test]
    fn a_check_error_is_bounded_and_says_nothing_of_the_values() {
        let long = AcpCheck::ended(AcpCheckState::Failed, Some("é".repeat(MAX_CHECK_ERROR)));
        assert!(long.failed());
        let error = long.error.0.expect("an error");
        assert!(error.len() <= MAX_CHECK_ERROR + '\u{2026}'.len_utf8(), "{}", error.len());
        let agent = AcpAgent {
            id: "x".into(),
            name: "X".into(),
            command: "/x".into(),
            args: vec![],
            env: vec![
                EnvVar { key: "SHORT".into(), value: "ab".into() },
                EnvVar { key: "KEY".into(), value: "secret-value".into() },
                EnvVar { key: "LONGER".into(), value: "secret-value-2".into() },
            ],
            last_check: None,
        };
        assert_eq!(
            agent.without_values("ab: secret-value-2, then secret-value"),
            "ab: [redacted], then [redacted]",
            "the longest first, the short ones left alone"
        );
    }

    /// What the agent says of itself is kept and listed: it is redacted as
    /// its errors are, and no longer than a name.
    #[test]
    fn what_a_ready_agent_says_of_itself_is_redacted_and_bounded() {
        let answer = crate::acp::InitializeResponse::read(&serde_json::json!({
            "protocolVersion": 1,
            "agentInfo": {"name": "agent", "title": "for key-1234567890", "version": "x".repeat(MAX_AGENT_WORD * 2)},
            "authMethods": (0..MAX_AUTH_METHODS * 2).map(|i| serde_json::json!({"id": format!("m{i}"), "name": "key-1234567890"})).collect::<Vec<_>>(),
        }));
        let check = AcpCheck::ready(PathBuf::from("/x/agent"), &answer, |text| text.replace("key-1234567890", "[redacted]"));
        let info = check.agent_info.0.expect("what it is");
        assert_eq!(info.title.0.as_deref(), Some("for [redacted]"));
        assert_eq!(info.version.0.map(|version| version.chars().count()), Some(MAX_AGENT_WORD));
        let methods = check.auth_methods.0.expect("ways to sign in");
        assert_eq!(methods.len(), MAX_AUTH_METHODS);
        assert!(methods.iter().all(|method| method.name == "[redacted]"), "{methods:?}");
    }

    /// A last check's error is the agent's own words. The list, which every
    /// device may read, never carries it, whoever asks; the check's own route
    /// does, and a device reaches that only when it manages this computer.
    #[tokio::test]
    async fn a_check_error_is_only_on_the_checks_own_route() {
        use crate::agent::bridge::{refusal, Standing, View};
        use tower_service::Service;

        async fn get(router: &mut gotcha::axum::Router, path: &str, headers: &[(&str, &str)]) -> (u16, serde_json::Value) {
            let mut request = gotcha::axum::http::Request::get(path).header("host", "127.0.0.1:8700");
            for (name, value) in headers {
                request = request.header(*name, *value);
            }
            let mut request = request.body(gotcha::axum::body::Body::empty()).expect("request");
            let peer: std::net::SocketAddr = "127.0.0.1:50000".parse().expect("peer");
            request.extensions_mut().insert(gotcha::axum::extract::ConnectInfo(peer));
            let response = router.call(request).await.expect("infallible");
            let status = response.status().as_u16();
            let body = gotcha::axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("body");
            (status, serde_json::from_slice(&body).expect("json"))
        }

        let home = tempfile::tempdir().expect("tempdir");
        let (state, mut router) = crate::api::test_support::guarded_router(home.path(), Some(false)).await;
        state.paths.update_acp_agent("x", &patch(Some("/x/agent"), None, None)).expect("saved");
        let failed = AcpCheck::ended(AcpCheckState::Failed, Some("the agent's own words".into()));
        assert!(state.paths.acp_agent("x").unwrap().record_check(&state.paths, &failed));
        let credential = state.drive.auth.internal_credential().to_string();
        let device = [
            (crate::agent::bridge::CREDENTIAL_HEADER, credential.as_str()),
            (crate::agent::bridge::DEVICE_HEADER, "d-1"),
        ];

        // This computer, and a device bridged in: how the check ended, not why.
        for headers in [&[][..], &device[..]] {
            let (status, list) = get(&mut router, "/api/acp/agents", headers).await;
            assert_eq!(status, 200, "{list:#}");
            let check = &list["agents"][0]["lastCheck"];
            assert_eq!(check["state"], "failed", "{list:#}");
            assert!(check["checkedAt"].is_string(), "{list:#}");
            assert!(check.get("error").is_none(), "{list:#}");
        }

        // The bridge lets every device list the agents, and only one that
        // manages this computer ask for the check in full.
        let plain = Standing {
            admin: false,
            view: View::Fresh,
            controls: true,
        };
        let admin = Standing { admin: true, ..plain };
        assert_eq!(refusal("GET", "/api/acp/agents", plain), None);
        assert_eq!(refusal("GET", "/api/acp/agents/x/check", plain).map(|refused| refused.answer().0), Some(403));
        assert_eq!(refusal("GET", "/api/acp/agents/x/check", admin), None);
        let (status, check) = get(&mut router, "/api/acp/agents/x/check", &device).await;
        assert_eq!((status, check["error"].as_str()), (200, Some("the agent's own words")), "{check:#}");

        state.paths.update_acp_agent("y", &patch(Some("/y/agent"), None, None)).expect("saved");
        assert_eq!(get(&mut router, "/api/acp/agents/y/check", &[]).await.0, 404, "never checked");
        assert_eq!(get(&mut router, "/api/acp/agents/z/check", &[]).await.0, 404, "no such agent");
    }

    #[test]
    fn a_preset_says_whether_its_executable_is_on_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let preset = presets().first().expect("a built-in").clone();
        let env = Env::from([("PATH".to_string(), dir.path().to_string_lossy().into_owned())]);
        assert!(!preset.looked_up(&env).on_path);
        let program = dir.path().join(&preset.command);
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(preset.looked_up(&env).on_path);
    }
}
