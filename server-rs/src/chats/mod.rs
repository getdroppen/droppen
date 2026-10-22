//! Instant chats: the casual-conversation surface, on two very different
//! backends behind one list.
//!
//! Port of the former TypeScript server's `chats.ts` plus the chat routes of
//! its `app.ts`. A chat is either
//!
//! * **CLI-backed** — an ordinary `claude` / `codex` session anchored in
//!   [`chats_cwd`], billed to the CLI's own subscription login. Nothing about it
//!   is stored here: it is a session like any other, and the only things that
//!   make it a chat are the anchor directory and the persona
//!   ([`ensure_chats_cwd`] for codex, `--system-prompt` for claude, both applied
//!   by [`crate::inject`]). Those sessions are hidden from `GET /api/sessions`
//!   and surface only through [`list_chats`].
//! * **API-backed** — messages remote-hand stores itself ([`store`]) and
//!   answers by streaming from any OpenAI-compatible endpoint the user
//!   configured ([`provider`]).
//!
//! The streaming send is the one route in this module without an `#[api]`
//! attribute: it answers with server-sent events, which the OpenAPI document
//! cannot describe, and the TypeScript leaves it out of the spec for the same
//! reason (the former TypeScript server's `app.ts`, "Server-sent events rather
//! than an OpenAPI route"). Its frames are therefore a contract of their own,
//! pinned by `tests/chats.rs`.

pub mod provider;
pub mod store;

use std::convert::Infallible;
use std::sync::Arc;

use gotcha::axum::body::Bytes;
use gotcha::axum::response::{IntoResponse, Response};
use gotcha::{api, Event, Json, KeepAlive, Schematic, Sse, State};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::api::error::ApiError;
use crate::api::extract::{ApiJson, ApiPath};
use crate::api::{ApiRouter, AppState, SendResult};
use crate::inject::{self, SendOptions};
use crate::openapi_ext::{ExtraResponses, Nullable, Optional};
use crate::sessions::{chats_cwd, claude, codex, Paths, Source};

pub use provider::{ChatModelList, ChatProviderInfo, ChatProviderUpdate};
pub use store::{ApiChat, ApiChatMessage};

/// The codex chat persona.
///
/// codex has no `--system-prompt`, so its persona is an `AGENTS.md` in the
/// anchor directory. The bytes are the contract: this is the text a CLI is
/// actually launched against.
const CHATS_AGENTS_MD: &str = "# Instant chat\n\
                               \n\
                               This directory is a casual chat app, NOT a coding session. The user wants a\n\
                               normal, friendly conversation. Answer in the user's language, keep it\n\
                               conversational, and never run commands, read files, or mention this file.\n";

/// `z.string().min(1).max(100_000)`, the bound on a chat message.
const MAX_TEXT: usize = 100_000;
/// `ChatModelField`'s length cap.
const MAX_MODEL: usize = 200;
/// The reasoning-effort cap.
const MAX_EFFORT: usize = 20;
/// How many frames may sit between the provider and the browser.
const STREAM_BUFFER: usize = 16;

/// Everything that can stop a chat before it starts.
///
/// The split mirrors the TypeScript's: a `ChatError` is a refusal whose text the
/// user is meant to read, while anything else escapes as a plain `Error` that
/// the route replaces with its own wording — so an `errno`, a URL or a header
/// never reaches the client.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ChatError {
    /// A refusal, reported verbatim.
    #[error("{0}")]
    Message(String),
    /// A failure the client is told nothing about.
    #[error("{0}")]
    Other(String),
}

impl ChatError {
    fn message(text: impl Into<String>) -> Self {
        ChatError::Message(text.into())
    }

    fn other(text: impl Into<String>) -> Self {
        ChatError::Other(text.into())
    }

    /// What the client is told, masking anything the TypeScript would mask.
    pub fn client_message(&self, fallback: &str) -> String {
        match self {
            ChatError::Message(text) => text.clone(),
            ChatError::Other(_) => fallback.to_string(),
        }
    }

    /// The same, as the `400` every chat route answers a refusal with.
    fn into_api(self, fallback: &str) -> ApiError {
        ApiError::bad_request(self.client_message(fallback))
    }
}

/// The subsystem's state: where things live, and how to reach the provider.
///
/// One `reqwest::Client` for the process, so the connection pool survives
/// between turns rather than a fresh TLS handshake per message.
pub struct Chats {
    paths: Arc<Paths>,
    http: reqwest::Client,
    /// Serializes the read-modify-write of both stores, the way the
    /// TypeScript's module-level `withLock` promise chain does.
    lock: tokio::sync::Mutex<()>,
}

impl Chats {
    /// Build the subsystem for one server instance.
    pub fn new(paths: Arc<Paths>) -> Arc<Self> {
        Arc::new(Self {
            paths,
            http: reqwest::Client::builder().build().expect("build the chat HTTP client"),
            lock: tokio::sync::Mutex::new(()),
        })
    }
}

/// Which backend a chat runs on.
///
/// One enum for all three of the contract's `"claude" | "codex" | "api"` fields
/// — `ChatNew.backend`, `ChatSummary.kind` and `ChatNewResult.kind` — because
/// they are the same three values and always agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum ChatKind {
    /// A `claude` CLI session in the anchor directory.
    Claude,
    /// A `codex` CLI session in the anchor directory.
    Codex,
    /// Messages remote-hand stores and a configured provider answers.
    Api,
}

impl ChatKind {
    /// The CLI this kind names, or `None` for the API backend.
    fn source(self) -> Option<Source> {
        match self {
            ChatKind::Claude => Some(Source::Claude),
            ChatKind::Codex => Some(Source::Codex),
            ChatKind::Api => None,
        }
    }

    /// The kind a discovered CLI session belongs to.
    fn of(source: Source) -> Self {
        match source {
            Source::Claude => ChatKind::Claude,
            Source::Codex => ChatKind::Codex,
        }
    }
}

/// One row of the chat list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ChatSummary {
    /// Which backend answers it.
    pub kind: ChatKind,
    /// The chat id — a session id for CLI chats, a uuid for API ones.
    pub id: String,
    /// Display name.
    pub title: String,
    /// The model, for API chats. CLI chats are answered by whatever the CLI is
    /// configured with, which remote-hand does not know, so it is `null`.
    pub model: Nullable<String>,
    /// When it last changed, ISO-8601.
    pub updated_at: String,
}

/// The chat list response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct ChatList {
    /// Every chat from both backends, newest first.
    pub chats: Vec<ChatSummary>,
}

/// The body of `POST /api/chats/new`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ChatNew {
    /// Which backend to start on.
    pub backend: ChatKind,
    /// The first message. Required for the CLI backends, because spawning the
    /// CLI *is* the first send; ignored by the API backend, which is created
    /// empty and sent to afterwards.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub text: Optional<String>,
    /// Model override.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub model: Optional<String>,
    /// Reasoning effort, for the CLI backends.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub effort: Optional<String>,
    /// Run a CLI chat under a stored account of its CLI; `null` means the local
    /// login. Absent follows the CLI's default — and, with none, skips a local
    /// login known to be signed out (see `accounts::rotation::start_for`).
    #[serde(default, deserialize_with = "crate::openapi_ext::double_option", skip_serializing_if = "Option::is_none")]
    pub account_id: Option<Option<String>>,
    /// `"start"`: `accountId` is the account `GET /api/accounts/start` named
    /// for a pair no model-less start reaches (`placed: false`), not a pick —
    /// logged as the start rule's choice and checked again at the spawn.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub account_from: Optional<crate::inject::AccountFrom>,
}

/// What `POST /api/chats/new` answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ChatNewResult {
    /// The backend the chat ended up on.
    pub kind: ChatKind,
    /// The new chat's id. Nullable in the contract because codex generates its
    /// own thread id, so there is a shape for "started, id not known yet".
    pub id: Nullable<String>,
}

/// Create the anchor directory and write the codex persona into it.
///
/// Called before every CLI chat spawn rather than once at boot, because the
/// directory is the user's own and can be deleted between two chats.
pub async fn ensure_chats_cwd(paths: &Paths) -> std::io::Result<std::path::PathBuf> {
    let cwd = chats_cwd(paths);
    tokio::fs::create_dir_all(&cwd).await?;
    tokio::fs::write(cwd.join("AGENTS.md"), CHATS_AGENTS_MD).await?;
    Ok(cwd)
}

// ------------------------------------------------------------------ validation

/// `ChatModelField`: `^[A-Za-z0-9][\w.:/-]*(?:\[[A-Za-z0-9]+\])?$`, at most 200
/// characters.
///
/// The optional bracketed tail is the variant tag
/// [`crate::drive::composer_prefs::MODEL_PATTERN`] already accepts on the send
/// routes: `claude-opus-5[1m]` is how the long-context variant of a model is
/// asked for, and it is what the model picker now offers, so a chat started
/// with the id the composer is showing has to be startable. It is matched as a
/// whole tag rather than by letting `[` and `]` into the character class,
/// because the value becomes a CLI argument and a class that admitted a bracket
/// anywhere would also admit an unbalanced one.
///
/// The `/` is this route's own: an API chat may name a provider-qualified model
/// (`anthropic/claude-sonnet-5`), which never reaches a CLI flag.
fn model_is_usable(model: &str) -> bool {
    let name = match model.strip_suffix(']') {
        Some(head) => match head.rsplit_once('[') {
            Some((name, tag)) if !tag.is_empty() && tag.chars().all(|c| c.is_ascii_alphanumeric()) => name,
            _ => return false,
        },
        None => model,
    };
    let mut chars = name.chars();
    let first = chars.next().is_some_and(|c| c.is_ascii_alphanumeric());
    let rest = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '-'));
    first && rest && model.chars().count() <= MAX_MODEL
}

/// Every bound `ChatNewSchema` puts on the body, as one `400`.
///
/// The values become CLI arguments, so the patterns are not cosmetic: an
/// unbounded `model` could read as a flag rather than as a model name.
fn validate_new(body: &ChatNew) -> Result<(), ApiError> {
    if let Some(text) = body.text.as_deref() {
        if text.is_empty() || text.chars().count() > MAX_TEXT {
            return Err(ApiError::bad_request("invalid text"));
        }
    }
    if let Some(model) = body.model.as_deref() {
        if !model_is_usable(model) {
            return Err(ApiError::bad_request("invalid model"));
        }
    }
    if let Some(effort) = body.effort.as_deref() {
        if effort.is_empty() || effort.chars().count() > MAX_EFFORT || !effort.chars().all(|c| c.is_ascii_lowercase()) {
            return Err(ApiError::bad_request("invalid effort"));
        }
    }
    // The session route's rule: a malformed id is a bad request; a well-formed
    // one that names nothing is refused by the spawn.
    if let Some(Some(id)) = &body.account_id {
        if !crate::accounts::store::is_uuid(id) {
            return Err(ApiError::bad_request(format!("invalid account id: {id}")));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------- routes

/// Instant chats from both backends, newest first.
#[api(id = "list_chats", group = "chats", summary = "List chats")]
pub async fn list_chats(State(state): State<AppState>) -> Json<ChatList> {
    let paths = state.paths.as_ref();
    let bindings = state.drive.accounts.session_account_map().await;
    let (claude_sessions, codex_sessions, api_chats) = tokio::join!(
        claude::list_claude_sessions(paths, &bindings),
        codex::list_codex_sessions(paths),
        store::list_api_chats(paths),
    );

    // The anchor directory is what makes a CLI session a chat; every other
    // session in the list belongs on the sessions screen instead.
    let anchor = chats_cwd(paths).to_string_lossy().into_owned();
    let mut chats: Vec<ChatSummary> = claude_sessions
        .into_iter()
        .chain(codex_sessions)
        .filter(|session| session.cwd.as_ref().map(String::as_str) == Some(anchor.as_str()))
        .map(|session| ChatSummary {
            kind: ChatKind::of(session.source),
            id: session.id,
            title: session.title,
            model: Nullable(None),
            updated_at: session.updated_at,
        })
        .collect();
    chats.extend(api_chats.into_iter().map(|chat| ChatSummary {
        kind: ChatKind::Api,
        id: chat.id,
        title: chat.title,
        model: Nullable(Some(chat.model)),
        updated_at: chat.updated_at,
    }));
    // `localeCompare` on ISO-8601 timestamps is a plain lexicographic compare.
    chats.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Json(ChatList { chats })
}

/// Start a chat. For the CLI backends the first message is already running when
/// this returns.
#[api(id = "new_chat", group = "chats", summary = "Start a chat")]
pub async fn new_chat(State(state): State<AppState>, ApiJson(body): ApiJson<ChatNew>) -> Result<Json<ChatNewResult>, ApiError> {
    validate_new(&body)?;

    let Some(source) = body.backend.source() else {
        let chat = state
            .chats
            .create_api_chat(body.model.as_deref())
            .await
            .map_err(|error| error.into_api("cannot create chat"))?;
        return Ok(Json(ChatNewResult {
            kind: ChatKind::Api,
            id: Nullable(Some(chat.id)),
        }));
    };

    let text = body.text.as_deref().unwrap_or_default();
    if text.trim().is_empty() {
        return Err(ApiError::bad_request("text is required for CLI chats"));
    }
    let cwd = ensure_chats_cwd(&state.paths)
        .await
        .map_err(|_| ApiError::bad_request("cannot create chat"))?;
    let options = SendOptions {
        model: body.model.0,
        effort: body.effort.0,
        chat: true,
        // A chat is billed to the login picked for it, else whichever one the
        // user made the default.
        account: crate::inject::AccountChoice::from_start_request(body.account_id, body.account_from.0),
        use_default_account: true,
        ..Default::default()
    };
    let id = inject::start_new_session(&state.drive, source, &cwd.to_string_lossy(), text, &options)
        .await
        .map_err(|error| ApiError::bad_request(error.client_message("cannot create chat")))?;
    // The chat list has to show the new session before its first transcript
    // line lands.
    state.global.request_refresh(None);
    Ok(Json(ChatNewResult {
        kind: body.backend,
        id: Nullable(Some(id)),
    }))
}

/// One API-backed chat, with every message.
#[api(id = "get_api_chat", group = "chats", summary = "Get an API chat")]
pub async fn get_api_chat(State(state): State<AppState>, ApiPath(id): ApiPath<String>) -> Result<Json<ApiChat>, ApiError> {
    store::read_api_chat(&state.paths, &id)
        .await
        .map(Json)
        .ok_or_else(|| ApiError::not_found("chat not found"))
}

/// Delete an API-backed chat.
#[api(id = "delete_api_chat", group = "chats", summary = "Delete an API chat")]
pub async fn delete_api_chat(State(state): State<AppState>, ApiPath(id): ApiPath<String>) -> Result<Json<SendResult>, ApiError> {
    if !state.chats.delete_api_chat(&id).await {
        return Err(ApiError::not_found("chat not found"));
    }
    Ok(Json(SendResult { accepted: true }))
}

/// The configured provider, with the key redacted.
#[api(id = "chat_provider", group = "chats", summary = "Chat provider")]
pub async fn chat_provider(State(state): State<AppState>) -> Json<ChatProviderInfo> {
    Json(state.chats.provider_info().await)
}

/// Update the provider configuration. An empty `apiKey` clears the stored key.
#[api(id = "update_chat_provider", group = "chats", summary = "Update the chat provider")]
pub async fn update_chat_provider(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<ChatProviderUpdate>,
) -> Result<Json<ChatProviderInfo>, ApiError> {
    state
        .chats
        .update_provider(&body)
        .await
        .map(Json)
        .map_err(|error| ApiError::internal(error.client_message("cannot save the provider configuration")))
}

/// Model ids from the provider's `/models` endpoint.
#[api(id = "chat_provider_models", group = "chats", summary = "Provider models")]
pub async fn chat_provider_models(State(state): State<AppState>) -> Result<Json<ChatModelList>, ApiError> {
    let models = state
        .chats
        .provider_models()
        .await
        .map_err(|error| error.into_api("cannot list provider models"))?;
    Ok(Json(ChatModelList { models }))
}

// ------------------------------------------------------------ the streaming send

/// A validated `POST /api/api-chats/{id}/message` body.
struct MessageRequest {
    text: String,
    model: Option<String>,
}

/// Parse and validate the send body, or `None` for the route's flat `400`.
///
/// Hand-rolled rather than an extractor because this route is not an OpenAPI
/// one: the TypeScript reads the body with `c.req.json().catch(() => null)` and
/// answers every failure — malformed JSON, a missing `text`, a `model` that
/// does not match the pattern — with the same `{"error":"invalid body"}`.
fn parse_message_body(raw: &[u8]) -> Option<MessageRequest> {
    let value = serde_json::from_slice::<Value>(raw).ok()?;
    let text = value.get("text")?.as_str()?;
    if text.is_empty() || text.chars().count() > MAX_TEXT {
        return None;
    }
    // `z.string().optional()` accepts an absent key, never a `null` one.
    let model = match value.get("model") {
        None => None,
        Some(model) => {
            let model = model.as_str()?;
            if !model_is_usable(model) {
                return None;
            }
            Some(model.to_string())
        }
    };
    Some(MessageRequest {
        text: text.to_string(),
        model,
    })
}

/// One `data:` frame.
fn frame(payload: &Value) -> Event {
    Event::default().data(payload.to_string())
}

/// The SSE stream's end of the channel, as something the store can push to.
struct EventSink(tokio::sync::mpsc::Sender<Event>);

impl store::DeltaSink for EventSink {
    async fn delta(&self, text: String) -> bool {
        self.0.send(frame(&json!({ "type": "delta", "text": text }))).await.is_ok()
    }
}

/// Send a message to an API-backed chat and stream the reply.
///
/// Deliberately **not** annotated with `#[api]`: the response is an event
/// stream, which neither gotcha's return-type derivation nor the golden
/// contract has a shape for — `openapi/openapi.json` documents the other six
/// chat routes and leaves this one out, so documenting it here would be a
/// difference rather than a fix.
///
/// The frames are `{"type":"delta","text":…}` per chunk, then either
/// `{"type":"done","content":…}` with the whole reply or
/// `{"type":"error","error":…}`; `web/src/features/chat/api-chat-view.tsx`
/// reads exactly those three.
///
/// The turn runs in its own task so the response can start streaming
/// immediately, and is bound to the stream's lifetime by a
/// [`CancellationToken`] drop guard: when the browser hangs up, the stream is
/// dropped, the guard fires, and the provider request is abandoned rather than
/// billed to the end.
pub async fn api_chat_message(State(state): State<AppState>, ApiPath(id): ApiPath<String>, body: Bytes) -> Response {
    let Some(request) = parse_message_body(&body) else {
        return ApiError::bad_request("invalid body").into_response();
    };

    let chats = state.chats.clone();
    let (sender, receiver) = tokio::sync::mpsc::channel::<Event>(STREAM_BUFFER);
    let cancel = CancellationToken::new();
    let guard = cancel.clone().drop_guard();

    tokio::spawn(async move {
        let sink = EventSink(sender.clone());
        let turn = chats.run_api_chat_turn(&id, &request.text, request.model.as_deref(), &sink);
        let closing = tokio::select! {
            biased;
            () = cancel.cancelled() => return,
            outcome = turn => match outcome {
                Ok(content) => frame(&json!({ "type": "done", "content": content })),
                Err(error) => frame(&json!({ "type": "error", "error": error.client_message("chat request failed") })),
            },
        };
        let _ = sender.send(closing).await;
    });

    // The guard rides along in the stream's state, so it is dropped exactly
    // when the stream is.
    let stream = futures_util::stream::unfold((receiver, guard), |(mut receiver, guard)| async move {
        receiver.recv().await.map(|event| (Ok::<Event, Infallible>(event), (receiver, guard)))
    });
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

/// Register this subsystem's routes.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router
        .get("/api/chats", list_chats)
        .post("/api/chats/new", new_chat)
        .get("/api/api-chats/{id}", get_api_chat)
        .delete("/api/api-chats/{id}", delete_api_chat)
        .post("/api/api-chats/{id}/message", api_chat_message)
        .get("/api/chat-provider", chat_provider)
        .put("/api/chat-provider", update_chat_provider)
        .get("/api/chat-provider/models", chat_provider_models)
}

/// Everything gotcha cannot express about those routes.
///
/// `PUT /api/chat-provider` uses [`ExtraResponses::describe_final`] because a
/// write failure is a `500` the golden contract does not document, so the
/// `default` response gotcha derives from the handler's `Result` has to go
/// without a status entry taking its place.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe("GET", "/api/chats", 200, "Instant chats from both backends, newest first")
        .describe(
            "POST",
            "/api/chats/new",
            200,
            "Chat created (claude: the first message is already running)",
        )
        .error("POST", "/api/chats/new", 400, "Invalid request or provider not configured")
        .describe("GET", "/api/api-chats/{id}", 200, "Full API-backed chat with messages")
        .error("GET", "/api/api-chats/{id}", 404, "Chat not found")
        .describe("DELETE", "/api/api-chats/{id}", 200, "Chat deleted")
        .error("DELETE", "/api/api-chats/{id}", 404, "Chat not found")
        .describe("GET", "/api/chat-provider", 200, "OpenAI-compatible provider config (key masked)")
        .describe_final("PUT", "/api/chat-provider", 200, "Updated provider config")
        .describe("GET", "/api/chat-provider/models", 200, "Model ids from the provider's /models endpoint")
        .error("GET", "/api/chat-provider/models", 400, "Provider not configured or unreachable");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_anchor_directory_carries_the_codex_persona() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());

        let cwd = ensure_chats_cwd(&paths).await.expect("create");
        assert_eq!(cwd, chats_cwd(&paths));
        let persona = std::fs::read_to_string(cwd.join("AGENTS.md")).expect("read");
        assert_eq!(
            persona,
            "# Instant chat\n\nThis directory is a casual chat app, NOT a coding session. The user wants a\n\
             normal, friendly conversation. Answer in the user's language, keep it\n\
             conversational, and never run commands, read files, or mention this file.\n",
            "the persona is what a codex chat is actually launched against"
        );
        assert!(persona.ends_with(".\n"), "the template literal ends with a newline");
    }

    #[tokio::test]
    async fn the_persona_is_rewritten_when_the_user_deletes_it() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());

        ensure_chats_cwd(&paths).await.expect("create");
        std::fs::write(chats_cwd(&paths).join("AGENTS.md"), "you are a pirate").expect("tamper");
        ensure_chats_cwd(&paths).await.expect("recreate");
        assert_eq!(
            std::fs::read_to_string(chats_cwd(&paths).join("AGENTS.md")).expect("read"),
            CHATS_AGENTS_MD,
            "every spawn restores the persona"
        );

        std::fs::remove_dir_all(chats_cwd(&paths)).expect("delete");
        assert!(ensure_chats_cwd(&paths).await.is_ok(), "a deleted anchor directory is recreated");
    }

    #[test]
    fn a_model_name_cannot_read_as_a_flag() {
        for model in ["gpt-4o", "anthropic/claude-sonnet-4.5", "a", "x_1:2/3.4-5"] {
            assert!(model_is_usable(model), "{model} should be usable");
        }
        for model in ["", "-rf", "--version", "a b", "a;rm", "a\n", &"x".repeat(MAX_MODEL + 1)] {
            assert!(!model_is_usable(model), "{model:?} must be refused");
        }
        assert!(model_is_usable(&"x".repeat(MAX_MODEL)), "the cap itself is allowed");
    }

    /// The picker offers the long-context aliases the CLI publishes, so the
    /// route a picked model is started through has to take them — the send
    /// routes' own pattern already does.
    #[test]
    fn the_long_context_variant_tag_is_accepted_here_too() {
        for model in ["opus[1m]", "claude-opus-5[1m]", "claude-opus-4-8[1M]", "a[0]"] {
            assert!(model_is_usable(model), "{model} is what the picker now offers");
        }
        for model in ["opus[1m", "opus1m]", "opus[]", "opus[1m]x", "opus[a b]", "a[b[c]", "[1m]"] {
            assert!(!model_is_usable(model), "{model:?} is not a tag, balanced or otherwise");
        }
    }

    #[test]
    fn the_send_body_is_accepted_exactly_where_zod_accepts_it() {
        let parsed = parse_message_body(br#"{"text":"hi"}"#).expect("minimal body");
        assert_eq!(parsed.text, "hi");
        assert_eq!(parsed.model, None);
        assert_eq!(
            parse_message_body(br#"{"text":"hi","model":"gpt-4o"}"#)
                .expect("with model")
                .model
                .as_deref(),
            Some("gpt-4o")
        );

        for body in [
            &b"not json"[..],
            b"null",
            b"{}",
            br#"{"text":""}"#,
            br#"{"text":42}"#,
            // `.optional()` means the key may be absent, not that it may be null.
            br#"{"text":"hi","model":null}"#,
            br#"{"text":"hi","model":"--dangerous"}"#,
        ] {
            assert!(
                parse_message_body(body).is_none(),
                "{:?} is not a valid body",
                String::from_utf8_lossy(body)
            );
        }
        let long = format!(r#"{{"text":"{}"}}"#, "x".repeat(MAX_TEXT + 1));
        assert!(parse_message_body(long.as_bytes()).is_none(), "the text cap is enforced");
    }

    #[test]
    fn the_new_chat_body_is_bounded() {
        let body = |text: Option<&str>, model: Option<&str>, effort: Option<&str>| ChatNew {
            backend: ChatKind::Api,
            text: Optional(text.map(str::to_owned)),
            model: Optional(model.map(str::to_owned)),
            effort: Optional(effort.map(str::to_owned)),
            account_id: None,
            account_from: Optional(None),
        };
        assert!(validate_new(&body(None, None, None)).is_ok());
        assert!(validate_new(&body(Some("hi"), Some("gpt-4o"), Some("high"))).is_ok());
        assert!(validate_new(&body(Some(""), None, None)).is_err(), "z.string().min(1)");
        assert!(validate_new(&body(None, Some("-rf"), None)).is_err());
        assert!(validate_new(&body(None, None, Some("HIGH"))).is_err(), "^[a-z]+$");
        assert!(validate_new(&body(None, None, Some("verylongeffortvalue!"))).is_err());
    }

    /// A CLI chat may name its login the way a new session does: absent follows
    /// the default, `null` is the local login, a uuid is an account.
    #[test]
    fn a_new_chat_names_its_login_like_a_new_session() {
        let parse = |raw: &str| serde_json::from_str::<ChatNew>(raw).expect("parse");
        assert_eq!(
            parse(r#"{"backend":"claude","text":"hi"}"#).account_id,
            None,
            "an older client's body still reads"
        );
        assert_eq!(parse(r#"{"backend":"claude","text":"hi","accountId":null}"#).account_id, Some(None));
        let named = parse(r#"{"backend":"claude","text":"hi","accountId":"11111111-2222-3333-4444-555555555555"}"#);
        assert_eq!(named.account_id, Some(Some("11111111-2222-3333-4444-555555555555".to_string())));
        assert!(validate_new(&named).is_ok());
        let malformed = parse(r#"{"backend":"claude","text":"hi","accountId":"../x"}"#);
        assert!(validate_new(&malformed).is_err(), "only a uuid can name an account");
    }

    #[test]
    fn the_three_backend_names_are_the_contract() {
        for (kind, name) in [(ChatKind::Claude, "claude"), (ChatKind::Codex, "codex"), (ChatKind::Api, "api")] {
            assert_eq!(serde_json::to_value(kind).expect("encode"), json!(name));
        }
        assert_eq!(ChatKind::of(Source::Claude), ChatKind::Claude);
        assert_eq!(ChatKind::of(Source::Codex), ChatKind::Codex);
        assert_eq!(ChatKind::Api.source(), None, "the API backend is not a CLI");
    }

    #[test]
    fn a_cli_chat_reports_no_model_and_an_api_chat_does() {
        let summary = ChatSummary {
            kind: ChatKind::Claude,
            id: "s1".into(),
            title: "hello".into(),
            model: Nullable(None),
            updated_at: "2024-05-01T10:00:00.000Z".into(),
        };
        assert_eq!(
            serde_json::to_value(&summary).expect("encode"),
            json!({ "kind": "claude", "id": "s1", "title": "hello", "model": null, "updatedAt": "2024-05-01T10:00:00.000Z" }),
            "`model` is present and null, never absent"
        );
    }
}
