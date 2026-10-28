//! The part of the [Agent Client Protocol](https://agentclientprotocol.com)
//! this server speaks, as Rust types.
//!
//! Defined here rather than taken from the official schema crate on purpose:
//! that crate turns on `serde_json`'s `preserve_order`, and Cargo features
//! apply to the whole build, so depending on it would change the key order of
//! every JSON document this server writes — the API contract and every
//! fingerprint taken over a re-serialized `Value` included.
//!
//! Only what [`crate::inject::acp`] sends or reads is here, and reading is
//! lenient the way ACP asks clients to be: an update kind, content block,
//! tool kind or stop reason this build does not know is kept as `Other`
//! rather than failing the message, and unknown fields are ignored.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Map, Value};

/// The protocol version this client speaks.
pub const PROTOCOL_VERSION: u64 = 1;

/// `session/update`: something an agent says about a session.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNotification {
    /// The session it is about.
    pub session_id: String,
    /// What it says.
    pub update: SessionUpdate,
}

/// The kinds of `session/update` this server records.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
pub enum SessionUpdate {
    /// A piece of the user's message: only ever replayed by `session/load`,
    /// unless the agent echoes what it was sent.
    UserMessageChunk(ContentChunk),
    /// A piece of the agent's answer.
    AgentMessageChunk(ContentChunk),
    /// A piece of the agent's reasoning.
    AgentThoughtChunk(ContentChunk),
    /// A tool call starting.
    ToolCall(ToolCall),
    /// A tool call progressing, or ending.
    ToolCallUpdate(ToolCallUpdate),
    /// How full the context window is.
    UsageUpdate(UsageUpdate),
    /// The session's configuration options changed: a model switch can bring
    /// a different set of efforts with it.
    ConfigOptionUpdate(ConfigOptionUpdate),
    /// Anything else: plans, mode and command lists, session info.
    #[serde(other)]
    Other,
}

/// One streamed piece of a message.
#[derive(Debug, Clone, Deserialize)]
pub struct ContentChunk {
    /// The piece itself.
    pub content: ContentBlock,
}

/// A piece of content, as far as this server shows it.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Text.
    Text {
        /// The text.
        text: String,
    },
    /// An image, which a transcript names rather than draws.
    Image {},
    /// A link to a resource the agent can read.
    ResourceLink {
        /// The resource's name.
        name: String,
    },
    /// Audio, an embedded resource, or a kind this build does not know.
    #[serde(other)]
    Other,
}

/// What a tool call is for: how the session view labels it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Reading files or data.
    Read,
    /// Changing files.
    Edit,
    /// Removing files.
    Delete,
    /// Moving or renaming files.
    Move,
    /// Searching.
    Search,
    /// Running a command.
    Execute,
    /// Thinking out loud.
    Think,
    /// Fetching from the network.
    Fetch,
    /// Anything else.
    #[default]
    #[serde(other)]
    Other,
}

/// Where a tool call is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    /// Not started yet.
    Pending,
    /// Running.
    InProgress,
    /// Finished.
    Completed,
    /// Finished, and failed.
    Failed,
    /// A status this build does not know.
    #[serde(other)]
    Other,
}

/// A tool call starting.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    /// The call's id, which its updates name.
    pub tool_call_id: String,
    /// What the agent calls it: a command line, a file name.
    #[serde(default)]
    pub title: String,
    /// What it is for.
    #[serde(default)]
    pub kind: ToolKind,
    /// The tool's input, as the agent gave it.
    #[serde(default)]
    pub raw_input: Option<Value>,
    /// Extension data: an agent may mark a context hand-off here.
    #[serde(default, rename = "_meta")]
    pub meta: Option<Map<String, Value>>,
}

/// A tool call progressing, or ending. Every field but the id is optional:
/// an update carries only what changed.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallUpdate {
    /// The call it updates.
    pub tool_call_id: String,
    /// Where it is now.
    #[serde(default)]
    pub status: Option<ToolCallStatus>,
    /// What it produced, for the user to read.
    #[serde(default)]
    pub content: Option<Vec<ToolCallContent>>,
    /// What it returned to the agent.
    #[serde(default)]
    pub raw_output: Option<Value>,
    /// Extension data: an agent may mark a context hand-off here.
    #[serde(default, rename = "_meta")]
    pub meta: Option<Map<String, Value>>,
}

/// Something a tool call produced.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolCallContent {
    /// Ordinary content.
    Content {
        /// The content itself.
        content: ContentBlock,
    },
    /// A file the call changed.
    Diff {
        /// The file.
        path: PathBuf,
    },
    /// A terminal, or a kind this build does not know.
    #[serde(other)]
    Other,
}

/// How full the context window is.
#[derive(Debug, Clone, Deserialize)]
pub struct UsageUpdate {
    /// Tokens in use.
    pub used: u64,
    /// The window's size.
    pub size: u64,
}

/// `config_option_update`: the session's options as they now stand.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigOptionUpdate {
    /// Every option, not only the ones that changed; read one by one by
    /// [`ConfigOptionUpdate::options`], so a kind this build skips does not
    /// cost the rest.
    #[serde(default)]
    pub config_options: Vec<Value>,
}

impl ConfigOptionUpdate {
    /// The options this build reads.
    pub fn options(&self) -> Vec<ConfigOption> {
        read_options(&self.config_options)
    }
}

/// What a configuration option is for. ACP names a few categories; an agent
/// may use others, and they are kept as they were spelled.
pub mod category {
    /// The model the session runs on.
    pub const MODEL: &str = "model";
    /// How hard the model thinks: Droppen's "effort".
    pub const THOUGHT_LEVEL: &str = "thought_level";
}

/// One of a session's configuration options, as `session/new`,
/// `session/load`, `session/set_config_option` and `config_option_update`
/// report it. Only selects are kept: a boolean option has nothing the composer
/// offers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigOption {
    /// What `session/set_config_option` names it by.
    pub id: String,
    /// What the user calls it.
    #[serde(default)]
    pub name: String,
    /// What it is for: [`category::MODEL`], [`category::THOUGHT_LEVEL`], …
    #[serde(default)]
    pub category: Option<String>,
    /// `select`, or a kind this build does not offer.
    #[serde(rename = "type", default)]
    pub kind: String,
    /// The value in force.
    #[serde(default)]
    pub current_value: Option<String>,
    /// What it may be set to, flat or in groups.
    #[serde(default)]
    pub options: Option<SelectOptions>,
}

/// A select's values: a flat list, or a list of named groups.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum SelectOptions {
    /// One flat list.
    Flat(Vec<SelectValue>),
    /// Named groups of values.
    Grouped(Vec<SelectGroup>),
}

/// One group of a select's values.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SelectGroup {
    /// The values in the group.
    pub options: Vec<SelectValue>,
}

/// One value a select may take.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SelectValue {
    /// What is sent.
    pub value: String,
    /// What is shown.
    #[serde(default)]
    pub name: String,
}

impl ConfigOption {
    /// Whether this is the option for `category`, by its category or, for an
    /// agent that names none, by the id ACP's examples use.
    pub fn is(&self, wanted: &str) -> bool {
        match self.category.as_deref() {
            Some(category) => category == wanted,
            None => self.id == wanted || (wanted == category::THOUGHT_LEVEL && self.id == "reasoning_effort"),
        }
    }

    /// Every value it may take, groups flattened, in the order given.
    pub fn values(&self) -> Vec<&SelectValue> {
        match &self.options {
            Some(SelectOptions::Flat(values)) => values.iter().collect(),
            Some(SelectOptions::Grouped(groups)) => groups.iter().flat_map(|group| group.options.iter()).collect(),
            None => Vec::new(),
        }
    }

    /// Whether `value` is one it may take.
    pub fn accepts(&self, value: &str) -> bool {
        self.values().iter().any(|candidate| candidate.value == value)
    }

    /// This, with the first [`MAX_SELECT_VALUES`] of its values.
    fn bounded(mut self) -> Self {
        match &mut self.options {
            Some(SelectOptions::Flat(values)) => values.truncate(MAX_SELECT_VALUES),
            Some(SelectOptions::Grouped(groups)) => {
                let mut left = MAX_SELECT_VALUES;
                groups.retain_mut(|group| {
                    group.options.truncate(left);
                    left -= group.options.len();
                    !group.options.is_empty()
                });
            }
            None => {}
        }
        self
    }
}

/// The select for `category` among `options`, if the agent offers one.
pub fn option_for<'a>(options: &'a [ConfigOption], category: &str) -> Option<&'a ConfigOption> {
    options.iter().find(|option| option.kind == "select" && option.is(category))
}

/// Read the `configOptions` of an answer, leniently: one this build cannot
/// read is skipped, not the whole list.
pub fn config_options(answer: &Value) -> Vec<ConfigOption> {
    answer["configOptions"].as_array().map(|options| read_options(options)).unwrap_or_default()
}

/// The most options a session's list keeps, and the most values one of them
/// keeps: every change to a session copies its list, and the composer offers
/// all of it.
const MAX_CONFIG_OPTIONS: usize = 32;
const MAX_SELECT_VALUES: usize = 256;

/// The options among `values` this build reads, as many as are kept.
fn read_options(values: &[Value]) -> Vec<ConfigOption> {
    values
        .iter()
        .filter_map(|option| serde_json::from_value::<ConfigOption>(option.clone()).ok())
        .take(MAX_CONFIG_OPTIONS)
        .map(ConfigOption::bounded)
        .collect()
}

/// The answer to `initialize`: what the agent is and says it can do.
///
/// Every part is optional to read: an agent that leaves one out, or spells
/// one in a way this build does not know, is described with what it did say.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResponse {
    /// The protocol version it agreed to.
    #[serde(default)]
    pub protocol_version: Option<u64>,
    /// What it can do.
    #[serde(default)]
    pub agent_capabilities: AgentCapabilities,
    /// The ways it can be signed in to, when it needs that before a session.
    #[serde(default)]
    pub auth_methods: Vec<AuthMethod>,
    /// Its own name and version.
    #[serde(default)]
    pub agent_info: Option<Implementation>,
}

impl InitializeResponse {
    /// Read an `initialize` answer, leniently: a part this build cannot read
    /// is left out rather than failing the rest.
    pub fn read(answer: &Value) -> Self {
        fn part<T: serde::de::DeserializeOwned>(answer: &Value, key: &str) -> Option<T> {
            answer.get(key).cloned().and_then(|value| serde_json::from_value(value).ok())
        }
        Self {
            protocol_version: answer.get("protocolVersion").and_then(Value::as_u64),
            agent_capabilities: part(answer, "agentCapabilities").unwrap_or_default(),
            auth_methods: part(answer, "authMethods").unwrap_or_default(),
            agent_info: part(answer, "agentInfo"),
        }
    }
}

/// `agentCapabilities`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentCapabilities {
    /// It can reopen a session (`session/load`).
    pub load_session: bool,
    /// What a prompt may hold besides text.
    pub prompt_capabilities: PromptCapabilities,
    /// Which MCP transports it can be given servers over, besides stdio.
    pub mcp_capabilities: McpCapabilities,
}

/// `promptCapabilities`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PromptCapabilities {
    /// Images.
    pub image: bool,
    /// Audio.
    pub audio: bool,
    /// Resources embedded in the prompt.
    pub embedded_context: bool,
}

/// `mcpCapabilities`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct McpCapabilities {
    /// MCP servers over HTTP.
    pub http: bool,
    /// MCP servers over SSE.
    pub sse: bool,
}

/// One of `authMethods`.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthMethod {
    /// Its id.
    pub id: String,
    /// What it is called.
    pub name: String,
    /// What it does.
    #[serde(default)]
    pub description: Option<String>,
}

/// `agentInfo`.
#[derive(Debug, Clone, Deserialize)]
pub struct Implementation {
    /// Its programmatic name.
    pub name: String,
    /// Its display name.
    #[serde(default)]
    pub title: Option<String>,
    /// Its version.
    #[serde(default)]
    pub version: Option<String>,
}

/// The answer to `session/prompt`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResponse {
    /// Why the turn ended.
    pub stop_reason: StopReason,
}

/// Why a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The agent finished.
    EndTurn,
    /// The model's output limit was reached.
    MaxTokens,
    /// Too many model requests in one turn.
    MaxTurnRequests,
    /// The agent refused to continue.
    Refusal,
    /// The client cancelled the turn.
    Cancelled,
    /// A reason this build does not know.
    #[serde(other)]
    Other,
}

impl StopReason {
    /// ACP's name for it, as a transcript records it.
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::EndTurn => "end_turn",
            StopReason::MaxTokens => "max_tokens",
            StopReason::MaxTurnRequests => "max_turn_requests",
            StopReason::Refusal => "refusal",
            StopReason::Cancelled => "cancelled",
            StopReason::Other => "other",
        }
    }
}

/// `fs/read_text_file`.
#[derive(Debug, Clone, Deserialize)]
pub struct ReadTextFileParams {
    /// The file, absolute.
    pub path: PathBuf,
    /// The first line to read, 1-based.
    #[serde(default)]
    pub line: Option<u32>,
    /// How many lines to read.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `fs/write_text_file`.
#[derive(Debug, Clone, Deserialize)]
pub struct WriteTextFileParams {
    /// The file, absolute.
    pub path: PathBuf,
    /// Its whole new content.
    pub content: String,
}

/// One environment variable a terminal is given.
#[derive(Debug, Clone, Deserialize)]
pub struct EnvVariable {
    /// Its name.
    pub name: String,
    /// Its value.
    pub value: String,
}

/// `terminal/create`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTerminalParams {
    /// The executable.
    pub command: String,
    /// Its arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Variables on top of the client's environment.
    #[serde(default)]
    pub env: Vec<EnvVariable>,
    /// Where it runs; the session's directory when absent.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// How much output to keep.
    #[serde(default)]
    pub output_byte_limit: Option<u64>,
}

/// `terminal/output`, `terminal/wait_for_exit`, `terminal/kill` and
/// `terminal/release`: which terminal.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalParams {
    /// The terminal `terminal/create` answered with.
    pub terminal_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_list_of_options_or_values_is_cut() {
        let values: Vec<Value> = (0..MAX_SELECT_VALUES * 2).map(|i| json!({"value": format!("m{i}"), "name": "M"})).collect();
        let option = |i: usize| json!({"id": format!("o{i}"), "name": "O", "type": "select", "currentValue": "m0", "options": values});
        let options = config_options(&json!({"configOptions": (0..MAX_CONFIG_OPTIONS * 2).map(option).collect::<Vec<_>>()}));
        assert_eq!(options.len(), MAX_CONFIG_OPTIONS);
        assert!(options.iter().all(|option| option.values().len() == MAX_SELECT_VALUES));
        let grouped = json!({"configOptions": [{"id": "g", "name": "G", "type": "select", "options": [{"options": values}, {"options": values}]}]});
        assert_eq!(config_options(&grouped)[0].values().len(), MAX_SELECT_VALUES, "across its groups");
    }
    use serde_json::json;

    #[test]
    fn what_this_build_does_not_know_is_kept_as_other() {
        let plan: SessionNotification =
            serde_json::from_value(json!({"sessionId": "s", "update": {"sessionUpdate": "plan", "entries": []}})).unwrap();
        assert!(matches!(plan.update, SessionUpdate::Other));
        let audio: ContentBlock = serde_json::from_value(json!({"type": "audio", "data": "", "mimeType": "audio/wav"})).unwrap();
        assert!(matches!(audio, ContentBlock::Other));
        let call: ToolCall = serde_json::from_value(json!({"toolCallId": "t", "kind": "switch_mode"})).unwrap();
        assert_eq!(call.kind, ToolKind::Other);
        let stop: PromptResponse = serde_json::from_value(json!({"stopReason": "something_new", "usage": {}})).unwrap();
        assert_eq!(stop.stop_reason, StopReason::Other);
    }

    #[test]
    fn configuration_options_are_read_flat_or_grouped() {
        let answer = json!({"configOptions": [
            {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "a:x",
             "options": [{"value": "a:x", "name": "X"}, {"value": "openrouter:b/y", "name": "Y"}]},
            {"id": "reasoning_effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": "auto",
             "options": [{"group": "g", "name": "G", "options": [{"value": "auto", "name": "Auto"}, {"value": "high", "name": "High"}]}]},
            {"id": "fast", "name": "Fast", "type": "boolean", "currentValue": false},
            {"not": "an option"},
        ]});
        let options = config_options(&answer);
        assert_eq!(options.len(), 2, "the boolean and the unreadable one are skipped: {options:?}");
        let model = option_for(&options, category::MODEL).expect("a model option");
        assert!(model.accepts("openrouter:b/y"));
        let effort = option_for(&options, category::THOUGHT_LEVEL).expect("an effort option");
        assert_eq!(effort.values().iter().map(|v| v.value.as_str()).collect::<Vec<_>>(), vec!["auto", "high"]);
        assert_eq!(effort.current_value.as_deref(), Some("auto"));
        let update: SessionUpdate =
            serde_json::from_value(json!({"sessionUpdate": "config_option_update", "configOptions": answer["configOptions"]})).unwrap();
        let SessionUpdate::ConfigOptionUpdate(update) = update else {
            panic!("a config option update")
        };
        assert_eq!(update.options(), options, "the same lenient reading");
    }

    #[test]
    fn a_tool_call_update_carries_only_what_changed() {
        let update: SessionUpdate = serde_json::from_value(json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "t", "status": "completed",
            "content": [{"type": "content", "content": {"type": "text", "text": "ok"}}, {"type": "terminal", "terminalId": "x"}],
        }))
        .unwrap();
        let SessionUpdate::ToolCallUpdate(update) = update else {
            panic!("a tool call update")
        };
        assert_eq!(update.status, Some(ToolCallStatus::Completed));
        assert!(matches!(
            update.content.as_deref(),
            Some([ToolCallContent::Content { .. }, ToolCallContent::Other])
        ));
        assert!(update.raw_output.is_none());
    }
}
