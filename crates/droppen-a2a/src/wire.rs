//! The A2A v1.0 objects as their JSON is written (`specification/a2a.proto`,
//! ProtoJSON).
//!
//! Written here rather than taken from the official SDK (`docs/a2a.md` §5 says
//! why). The rules are ProtoJSON's: field names are lowerCamelCase on the way
//! out, and a reader accepts the proto's own snake_case name too (the TCK's
//! JSON-RPC client sends `history_length`); enums are their names
//! (`TASK_STATE_WORKING`), and a reader takes the number as well; a field at
//! its default (an empty string or list, `false`) is left out, except where a
//! field is required. Objects carry no other keys: the TCK's schema allows
//! none.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// A JSON object, as a `google.protobuf.Struct`.
pub type Struct = Map<String, Value>;

fn is_false(value: &bool) -> bool {
    !*value
}

/// A task's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, gotcha::Schematic)]
pub enum TaskState {
    /// Not known.
    #[default]
    #[serde(rename = "TASK_STATE_UNSPECIFIED")]
    Unspecified,
    /// Accepted, and not yet running: starting, queued, or waiting for a slot.
    #[serde(rename = "TASK_STATE_SUBMITTED")]
    Submitted,
    /// Its turn is running.
    #[serde(rename = "TASK_STATE_WORKING")]
    Working,
    /// Its turn finished. Terminal.
    #[serde(rename = "TASK_STATE_COMPLETED")]
    Completed,
    /// It failed. Terminal.
    #[serde(rename = "TASK_STATE_FAILED")]
    Failed,
    /// It was canceled. Terminal.
    #[serde(rename = "TASK_STATE_CANCELED")]
    Canceled,
    /// It waits on an answer. Interrupted.
    #[serde(rename = "TASK_STATE_INPUT_REQUIRED")]
    InputRequired,
    /// The entry would not run it. Terminal.
    #[serde(rename = "TASK_STATE_REJECTED")]
    Rejected,
    /// It waits on authentication. Interrupted; never used here.
    #[serde(rename = "TASK_STATE_AUTH_REQUIRED")]
    AuthRequired,
}

impl TaskState {
    const ALL: [TaskState; 9] = [
        TaskState::Unspecified,
        TaskState::Submitted,
        TaskState::Working,
        TaskState::Completed,
        TaskState::Failed,
        TaskState::Canceled,
        TaskState::InputRequired,
        TaskState::Rejected,
        TaskState::AuthRequired,
    ];

    /// The ProtoJSON name.
    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Unspecified => "TASK_STATE_UNSPECIFIED",
            TaskState::Submitted => "TASK_STATE_SUBMITTED",
            TaskState::Working => "TASK_STATE_WORKING",
            TaskState::Completed => "TASK_STATE_COMPLETED",
            TaskState::Failed => "TASK_STATE_FAILED",
            TaskState::Canceled => "TASK_STATE_CANCELED",
            TaskState::InputRequired => "TASK_STATE_INPUT_REQUIRED",
            TaskState::Rejected => "TASK_STATE_REJECTED",
            TaskState::AuthRequired => "TASK_STATE_AUTH_REQUIRED",
        }
    }

    /// The state a name spells: the ProtoJSON name, or the short one the
    /// management API's filter also takes (`working`, `input_required`).
    pub fn parse(name: &str) -> Option<Self> {
        let upper = name.trim().to_ascii_uppercase().replace('-', "_");
        TaskState::ALL
            .into_iter()
            .find(|state| state.as_str() == upper || state.as_str().strip_prefix("TASK_STATE_") == Some(upper.as_str()))
    }

    /// Completed, failed, canceled or rejected: nothing changes it any more.
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskState::Completed | TaskState::Failed | TaskState::Canceled | TaskState::Rejected)
    }

    /// Terminal or interrupted: where a blocking send returns and a stream
    /// closes (v1.0 §3.2.2, §11.7).
    pub fn is_settled(self) -> bool {
        self.is_terminal() || matches!(self, TaskState::InputRequired | TaskState::AuthRequired)
    }
}

impl<'de> Deserialize<'de> for TaskState {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::String(name) if name.is_empty() => Ok(TaskState::Unspecified),
            Value::String(name) => TaskState::ALL
                .into_iter()
                .find(|state| state.as_str() == name)
                .ok_or_else(|| D::Error::custom(format!("unknown task state {name:?}"))),
            Value::Number(number) => number
                .as_u64()
                .and_then(|n| TaskState::ALL.get(usize::try_from(n).ok()?).copied())
                .ok_or_else(|| D::Error::custom(format!("unknown task state {number}"))),
            Value::Null => Ok(TaskState::Unspecified),
            other => Err(D::Error::custom(format!("a task state is a name, not {other}"))),
        }
    }
}

/// Who sent a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Role {
    /// Not said.
    #[default]
    Unspecified,
    /// The client.
    User,
    /// This agent.
    Agent,
}

impl Serialize for Role {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            Role::Unspecified => "ROLE_UNSPECIFIED",
            Role::User => "ROLE_USER",
            Role::Agent => "ROLE_AGENT",
        })
    }
}

impl<'de> Deserialize<'de> for Role {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::String(name) => match name.as_str() {
                "ROLE_USER" => Ok(Role::User),
                "ROLE_AGENT" => Ok(Role::Agent),
                "ROLE_UNSPECIFIED" | "" => Ok(Role::Unspecified),
                other => Err(D::Error::custom(format!("unknown role {other:?}"))),
            },
            Value::Number(number) => match number.as_u64() {
                Some(0) => Ok(Role::Unspecified),
                Some(1) => Ok(Role::User),
                Some(2) => Ok(Role::Agent),
                _ => Err(D::Error::custom(format!("unknown role {number}"))),
            },
            Value::Null => Ok(Role::Unspecified),
            other => Err(D::Error::custom(format!("a role is a name, not {other}"))),
        }
    }
}

/// What a part holds: exactly one of the four.
#[derive(Debug, Clone, PartialEq)]
pub enum PartContent {
    /// Text.
    Text(String),
    /// A file's bytes (base64 on the wire).
    Raw(Vec<u8>),
    /// A file by URL.
    Url(String),
    /// Any JSON value.
    Data(Value),
}

/// One piece of a message or an artifact.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    /// What it holds.
    pub content: PartContent,
    /// Extension metadata.
    pub metadata: Option<Struct>,
    /// A file name.
    pub filename: String,
    /// The media type; empty when not said.
    pub media_type: String,
}

impl Part {
    /// A text part.
    pub fn text(text: impl Into<String>) -> Self {
        Part {
            content: PartContent::Text(text.into()),
            metadata: None,
            filename: String::new(),
            media_type: String::new(),
        }
    }

    /// A data part, `application/json`.
    pub fn data(value: Value) -> Self {
        Part {
            content: PartContent::Data(value),
            metadata: None,
            filename: String::new(),
            media_type: "application/json".to_string(),
        }
    }

    /// This part with `media_type`.
    pub fn with_media_type(mut self, media_type: &str) -> Self {
        self.media_type = media_type.to_string();
        self
    }
}

/// The base64 a `raw` part is spelled in: standard, padded (ProtoJSON `bytes`).
fn base64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

impl Serialize for Part {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use base64::Engine as _;
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        match &self.content {
            PartContent::Text(text) => map.serialize_entry("text", text)?,
            PartContent::Raw(bytes) => map.serialize_entry("raw", &base64().encode(bytes))?,
            PartContent::Url(url) => map.serialize_entry("url", url)?,
            PartContent::Data(value) => map.serialize_entry("data", value)?,
        }
        if let Some(metadata) = &self.metadata {
            map.serialize_entry("metadata", metadata)?;
        }
        if !self.filename.is_empty() {
            map.serialize_entry("filename", &self.filename)?;
        }
        if !self.media_type.is_empty() {
            map.serialize_entry("mediaType", &self.media_type)?;
        }
        map.end()
    }
}

/// A value that was present, `null` included.
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

impl<'de> Deserialize<'de> for Part {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use base64::Engine as _;
        #[derive(Deserialize)]
        struct Wire {
            text: Option<String>,
            raw: Option<String>,
            url: Option<String>,
            #[serde(default, deserialize_with = "present")]
            data: Option<Value>,
            metadata: Option<Struct>,
            #[serde(default)]
            filename: Option<String>,
            #[serde(default, rename = "mediaType", alias = "media_type")]
            media_type: Option<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let mut contents = Vec::new();
        if let Some(text) = wire.text {
            contents.push(PartContent::Text(text));
        }
        if let Some(raw) = wire.raw {
            let bytes = base64()
                .decode(raw.as_bytes())
                .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(raw.as_bytes()))
                .map_err(|_| D::Error::custom("a raw part is base64"))?;
            contents.push(PartContent::Raw(bytes));
        }
        if let Some(url) = wire.url {
            contents.push(PartContent::Url(url));
        }
        if let Some(data) = wire.data {
            contents.push(PartContent::Data(data));
        }
        if contents.len() != 1 {
            return Err(D::Error::custom("a part holds exactly one of text, raw, url and data"));
        }
        Ok(Part {
            content: contents.remove(0),
            metadata: wire.metadata,
            filename: wire.filename.unwrap_or_default(),
            media_type: wire.media_type.unwrap_or_default(),
        })
    }
}

/// One message between client and agent.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// Minted by whoever wrote it.
    #[serde(default, alias = "message_id")]
    pub message_id: String,
    /// The context it belongs to.
    #[serde(default, alias = "context_id", skip_serializing_if = "String::is_empty")]
    pub context_id: String,
    /// The task it belongs to.
    #[serde(default, alias = "task_id", skip_serializing_if = "String::is_empty")]
    pub task_id: String,
    /// Who wrote it.
    #[serde(default)]
    pub role: Role,
    /// What it says.
    #[serde(default)]
    pub parts: Vec<Part>,
    /// Extension metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Struct>,
    /// Extensions it uses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,
    /// Tasks it refers to.
    #[serde(default, alias = "reference_task_ids", skip_serializing_if = "Vec::is_empty")]
    pub reference_task_ids: Vec<String>,
}

/// A task's state, and the message that goes with it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatus {
    /// The state.
    #[serde(default)]
    pub state: TaskState,
    /// What the agent says about it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    /// When it was set, ISO-8601 UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// A task's output.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    /// Unique within the task.
    #[serde(default, alias = "artifact_id")]
    pub artifact_id: String,
    /// A name for a person.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// What it is.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Its content.
    #[serde(default)]
    pub parts: Vec<Part>,
    /// Extension metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Struct>,
    /// Extensions it uses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,
}

/// A task.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// Minted here.
    pub id: String,
    /// Its context: one session.
    #[serde(default, alias = "context_id", skip_serializing_if = "String::is_empty")]
    pub context_id: String,
    /// Where it stands.
    pub status: TaskStatus,
    /// Its output. Left out when there is none, and when a list was not asked
    /// for artifacts (`Some(vec![])` writes an empty list).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<Artifact>>,
    /// The messages it was sent, oldest first, as `historyLength` cuts them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<Message>>,
    /// Extension metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Struct>,
}

/// A change of a task's status, in a stream or a push.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusUpdateEvent {
    /// The task.
    pub task_id: String,
    /// Its context.
    pub context_id: String,
    /// The new status.
    pub status: TaskStatus,
    /// Extension metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Struct>,
}

/// An artifact made or added to, in a stream or a push.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskArtifactUpdateEvent {
    /// The task.
    pub task_id: String,
    /// Its context.
    pub context_id: String,
    /// The artifact, or the chunk of it.
    pub artifact: Artifact,
    /// Whether this adds to the artifact of the same id sent before.
    #[serde(default, skip_serializing_if = "is_false")]
    pub append: bool,
    /// Whether this is its last chunk.
    #[serde(default, skip_serializing_if = "is_false")]
    pub last_chunk: bool,
    /// Extension metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Struct>,
}

/// One item of a stream, and the body of a push: exactly one of four.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamResponse {
    /// The task as it stands.
    Task(Task),
    /// A message instead of a task.
    Message(Message),
    /// A status change.
    StatusUpdate(TaskStatusUpdateEvent),
    /// An artifact update.
    ArtifactUpdate(TaskArtifactUpdateEvent),
}

/// What `SendMessage` answers: a task, or a message instead of one.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SendMessageResponse {
    /// The task the message made or moved.
    Task(Task),
    /// A direct answer, with no task.
    Message(Message),
}

/// How the webhook of a push is called.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticationInfo {
    /// The HTTP authentication scheme, `Bearer` say.
    #[serde(default)]
    pub scheme: String,
    /// Its credentials.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub credentials: String,
}

/// A webhook a task's updates are pushed to.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPushNotificationConfig {
    /// Always empty here: routing is by URL.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant: String,
    /// Unique within the task; minted here when the client sends none.
    #[serde(default)]
    pub id: String,
    /// The task.
    #[serde(default, alias = "task_id")]
    pub task_id: String,
    /// The webhook.
    #[serde(default)]
    pub url: String,
    /// A token the webhook checks.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token: String,
    /// The webhook's credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication: Option<AuthenticationInfo>,
}

/// How a `SendMessage` is to be handled.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageConfiguration {
    /// Media types the client takes back.
    #[serde(default, alias = "accepted_output_modes")]
    pub accepted_output_modes: Vec<String>,
    /// A webhook for the new task.
    #[serde(
        default,
        alias = "task_push_notification_config",
        alias = "pushNotificationConfig",
        alias = "push_notification_config"
    )]
    pub task_push_notification_config: Option<TaskPushNotificationConfig>,
    /// How much history the answer carries.
    #[serde(default, alias = "history_length")]
    pub history_length: Option<i32>,
    /// Answer at once rather than when the task settles.
    #[serde(default, alias = "return_immediately")]
    pub return_immediately: bool,
}

/// `SendMessage`, `SendStreamingMessage`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageRequest {
    /// Ignored: routing is by URL.
    #[serde(default)]
    pub tenant: String,
    /// The message.
    #[serde(default)]
    pub message: Option<Message>,
    /// How to handle it.
    #[serde(default)]
    pub configuration: Option<SendMessageConfiguration>,
    /// Extension metadata.
    #[serde(default)]
    pub metadata: Option<Struct>,
}

/// `GetTask`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTaskRequest {
    /// Ignored.
    #[serde(default)]
    pub tenant: String,
    /// The task.
    #[serde(default)]
    pub id: String,
    /// How much history.
    #[serde(default, alias = "history_length")]
    pub history_length: Option<i32>,
}

/// `ListTasks`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTasksRequest {
    /// Ignored.
    #[serde(default)]
    pub tenant: String,
    /// Only this context's tasks.
    #[serde(default, alias = "context_id")]
    pub context_id: String,
    /// Only tasks in this state.
    #[serde(default)]
    pub status: Option<TaskState>,
    /// 1 to 100; 50 when not said.
    #[serde(default, alias = "page_size")]
    pub page_size: Option<i32>,
    /// Where the previous page stopped.
    #[serde(default, alias = "page_token")]
    pub page_token: String,
    /// How much history each task carries.
    #[serde(default, alias = "history_length")]
    pub history_length: Option<i32>,
    /// Only tasks whose status changed at or after this instant.
    #[serde(default, alias = "status_timestamp_after")]
    pub status_timestamp_after: Option<String>,
    /// Whether the tasks carry their artifacts.
    #[serde(default, alias = "include_artifacts")]
    pub include_artifacts: Option<bool>,
}

/// `ListTasks`' answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTasksResponse {
    /// This page, the most recently changed first.
    pub tasks: Vec<Task>,
    /// The next page's token; empty on the last page.
    pub next_page_token: String,
    /// The page size used.
    pub page_size: i32,
    /// How many tasks match, over every page.
    pub total_size: i32,
}

/// `CancelTask`, and `SubscribeToTask` (whose `metadata` is ignored).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskIdRequest {
    /// Ignored.
    #[serde(default)]
    pub tenant: String,
    /// The task.
    #[serde(default)]
    pub id: String,
    /// Extension metadata.
    #[serde(default)]
    pub metadata: Option<Struct>,
}

/// `Get`/`DeleteTaskPushNotificationConfig`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushConfigRequest {
    /// Ignored.
    #[serde(default)]
    pub tenant: String,
    /// The task.
    #[serde(default, alias = "task_id")]
    pub task_id: String,
    /// The configuration.
    #[serde(default)]
    pub id: String,
}

/// `ListTaskPushNotificationConfigs`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPushConfigsRequest {
    /// Ignored.
    #[serde(default)]
    pub tenant: String,
    /// The task.
    #[serde(default, alias = "task_id")]
    pub task_id: String,
    /// Ignored: a task has few.
    #[serde(default, alias = "page_size")]
    pub page_size: Option<i32>,
    /// Ignored.
    #[serde(default, alias = "page_token")]
    pub page_token: String,
}

/// `ListTaskPushNotificationConfigs`' answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPushConfigsResponse {
    /// Every configuration of the task.
    pub configs: Vec<TaskPushNotificationConfig>,
    /// Always empty: one page.
    pub next_page_token: String,
}

/// Where an interface answers.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInterface {
    /// Its URL.
    pub url: String,
    /// `JSONRPC` or `HTTP+JSON`.
    pub protocol_binding: String,
    /// Empty here.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tenant: String,
    /// `1.0`.
    pub protocol_version: String,
}

/// What the agent can do beyond the core.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    /// SSE streams.
    pub streaming: bool,
    /// Webhooks.
    pub push_notifications: bool,
    /// An extended card for an authenticated client.
    pub extended_agent_card: bool,
}

/// `httpAuthSecurityScheme`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpAuthSecurityScheme {
    /// `Bearer`.
    pub scheme: String,
}

/// A security scheme: only HTTP authentication here.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SecurityScheme {
    /// HTTP authentication.
    HttpAuthSecurityScheme(HttpAuthSecurityScheme),
}

/// A list of strings, as a security requirement's scopes.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct StringList {
    /// The strings.
    pub list: Vec<String>,
}

/// The schemes a request must satisfy.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SecurityRequirement {
    /// Scheme name → required scopes.
    pub schemes: Map<String, Value>,
}

/// A skill of the agent.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkill {
    /// Its id.
    pub id: String,
    /// Its name.
    pub name: String,
    /// What it does.
    pub description: String,
    /// Keywords.
    pub tags: Vec<String>,
}

/// The Agent Card.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    /// The entry's name.
    pub name: String,
    /// The entry's description.
    pub description: String,
    /// JSON-RPC first, then HTTP+JSON.
    pub supported_interfaces: Vec<AgentInterface>,
    /// Droppen's version.
    pub version: String,
    /// What it can do.
    pub capabilities: AgentCapabilities,
    /// The bearer scheme.
    pub security_schemes: Map<String, Value>,
    /// The bearer scheme, required.
    pub security_requirements: Vec<SecurityRequirement>,
    /// The media types the agent takes.
    pub default_input_modes: Vec<String>,
    /// The media types it answers in.
    pub default_output_modes: Vec<String>,
    /// One skill, `coding_session`.
    pub skills: Vec<AgentSkill>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_part_is_exactly_one_content_and_leaves_its_defaults_out() {
        let part: Part = serde_json::from_value(json!({"text": "hi"})).unwrap();
        assert_eq!(serde_json::to_value(&part).unwrap(), json!({"text": "hi"}));

        let raw: Part = serde_json::from_value(json!({"raw": "dGNr", "media_type": "image/png", "filename": "a.png"})).unwrap();
        assert_eq!(raw.content, PartContent::Raw(b"tck".to_vec()));
        assert_eq!(
            serde_json::to_value(&raw).unwrap(),
            json!({"raw": "dGNr", "filename": "a.png", "mediaType": "image/png"})
        );

        let null: Part = serde_json::from_value(json!({"data": null})).unwrap();
        assert_eq!(null.content, PartContent::Data(Value::Null), "a present null is data");

        assert!(serde_json::from_value::<Part>(json!({"text": "a", "url": "https://x"})).is_err());
        assert!(serde_json::from_value::<Part>(json!({"mediaType": "text/plain"})).is_err());
    }

    #[test]
    fn a_reader_takes_the_proto_names_and_the_numbers_too() {
        let request: ListTasksRequest =
            serde_json::from_value(json!({"context_id": "c", "page_size": 5, "include_artifacts": true, "status": 2})).unwrap();
        assert_eq!(request.context_id, "c");
        assert_eq!(request.page_size, Some(5));
        assert_eq!(request.include_artifacts, Some(true));
        assert_eq!(request.status, Some(TaskState::Working));

        let message: Message = serde_json::from_value(json!({"message_id": "m", "role": "ROLE_USER", "parts": []})).unwrap();
        assert_eq!(message.message_id, "m");
        assert_eq!(message.role, Role::User);
    }

    #[test]
    fn stream_items_are_one_keyed_object_in_camel_case() {
        let item = StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "t".into(),
            context_id: "c".into(),
            status: TaskStatus {
                state: TaskState::Completed,
                message: None,
                timestamp: Some("2026-11-25T00:00:00.000Z".into()),
            },
            metadata: None,
        });
        assert_eq!(
            serde_json::to_value(&item).unwrap(),
            json!({"statusUpdate": {"taskId": "t", "contextId": "c", "status": {"state": "TASK_STATE_COMPLETED", "timestamp": "2026-11-25T00:00:00.000Z"}}})
        );
        let artifact = StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
            task_id: "t".into(),
            context_id: "c".into(),
            artifact: Artifact {
                artifact_id: "summary".into(),
                parts: vec![Part::text("done")],
                ..Artifact::default()
            },
            append: false,
            last_chunk: true,
            metadata: None,
        });
        assert_eq!(
            serde_json::to_value(&artifact).unwrap(),
            json!({"artifactUpdate": {"taskId": "t", "contextId": "c", "artifact": {"artifactId": "summary", "parts": [{"text": "done"}]}, "lastChunk": true}})
        );
    }

    #[test]
    fn a_state_is_named_both_ways() {
        assert_eq!(TaskState::parse("working"), Some(TaskState::Working));
        assert_eq!(TaskState::parse("TASK_STATE_INPUT_REQUIRED"), Some(TaskState::InputRequired));
        assert_eq!(TaskState::parse("input-required"), Some(TaskState::InputRequired));
        assert_eq!(TaskState::parse("nonsense"), None);
        assert!(TaskState::InputRequired.is_settled() && !TaskState::InputRequired.is_terminal());
    }
}
