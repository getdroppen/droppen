//! JSON-RPC wire types and framing helpers for the Codex App Server.

use super::*;

/// A request to codex. Its app server speaks JSON-RPC 2.0 without the
/// `jsonrpc` member on the wire.
#[derive(Serialize)]
pub(super) struct Request<'a, P> {
    pub(super) id: &'a str,
    pub(super) method: &'a str,
    pub(super) params: P,
}

/// A notification to codex: a request without an id, which nothing answers.
#[derive(Serialize)]
pub(super) struct Notification<'a, P> {
    pub(super) method: &'a str,
    pub(super) params: P,
}

/// The params of a call that takes none, written `{}`.
#[derive(Serialize)]
pub(super) struct NoParams {}

/// The answer to a request codex makes of us, with the request's id echoed in
/// the type it came in.
#[derive(Serialize)]
pub(super) struct ErrorResponse<'a> {
    pub(super) id: &'a Value,
    pub(super) error: RpcError,
}

#[derive(Serialize)]
pub(super) struct RpcError {
    pub(super) code: i64,
    pub(super) message: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct InitializeParams {
    pub(super) client_info: ClientInfo,
}

#[derive(Serialize)]
pub(super) struct ClientInfo {
    pub(super) name: &'static str,
    pub(super) title: &'static str,
    pub(super) version: &'static str,
}

/// Codex never stops a headless turn to ask; see [`reject_request`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum ApprovalPolicy {
    Never,
}

/// The params of `thread/start`, and of `thread/resume` when they name the
/// thread.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadParams<'a> {
    pub(super) cwd: &'a str,
    pub(super) approval_policy: ApprovalPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) thread_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) model_provider: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) base_instructions: Option<&'static str>,
}

/// A thread answer's settings as [`State::settings`] keeps them: what codex
/// said, and `null` for any it left out.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadSettings<'a> {
    pub(super) model: &'a Value,
    pub(super) effort: &'a Value,
    pub(super) sandbox_policy: &'a Value,
}

/// A message's `turn/start` params as [`Message::params`] keeps them. The
/// thread and the message's id are added when it is sent, by
/// [`TurnStartOnThread`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TurnStartParams<'a> {
    pub(super) input: Vec<UserInput<'a>>,
    pub(super) approval_policy: ApprovalPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) effort: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) sandbox_policy: Option<SandboxPolicy>,
}

/// One item of a turn's input.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(super) enum UserInput<'a> {
    Text { text: String },
    LocalImage { path: &'a str },
}

/// The sandbox a turn runs in. A turn that names none runs in the thread's.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(super) enum SandboxPolicy {
    WorkspaceWrite,
    ReadOnly,
    DangerFullAccess,
}

/// `turn/start`'s params as sent: a queued message's params exactly as the
/// journal holds them, whichever build wrote them, then the thread and the
/// message they start.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TurnStartOnThread<'a> {
    #[serde(flatten)]
    pub(super) params: StoredParams<'a>,
    pub(super) thread_id: &'a str,
    pub(super) client_user_message_id: &'a str,
}

/// A queued message's stored params, less the `threadId` and
/// `clientUserMessageId` [`TurnStartOnThread`] writes after them: a row that
/// holds its own must not name either twice, and the ones written now win.
pub(super) struct StoredParams<'a>(pub(super) &'a Value);

impl Serialize for StoredParams<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Object(params) => serializer.collect_map(
                params
                    .iter()
                    .filter(|(key, _)| !matches!(key.as_str(), "threadId" | "clientUserMessageId")),
            ),
            other => other.serialize(serializer),
        }
    }
}

/// `turn/steer`'s params: a queued message's input, into the turn it expects
/// to be running.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TurnSteerParams<'a> {
    pub(super) thread_id: &'a str,
    pub(super) expected_turn_id: Option<&'a str>,
    pub(super) input: &'a Value,
    pub(super) client_user_message_id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TurnInterruptParams<'a> {
    pub(super) thread_id: &'a str,
    pub(super) turn_id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadArchiveParams<'a> {
    pub(super) thread_id: &'a str,
}

#[cfg(test)]
pub(super) fn message(text: &str, options: &SendOptions) -> Result<Message, InjectError> {
    let wire_text = with_codex_files(text, &options.files);
    build_message(text, &wire_text, &options.images, None, options)
}

/// Validate uploaded attachments and assemble one queued Codex message.
///
/// * **Chats** (`options.chat`): images are validated by magic bytes and size
///   (refused if invalid/oversized) and non-image files are inlined as UTF-8
///   text blocks so read-only / tool-less instant chats can read them.
/// * **Coding sessions**: valid PNG/JPEG/GIF/WebP images within caps travel as
///   native `localImage` items; oversized or unsupported image formats fall
///   back to the file path suffix so Codex can still inspect them. Escapes and
///   hardlinks always fail loudly.
pub(super) async fn prepare_message(
    paths: &droppen_instance::paths::Paths,
    text: &str,
    options: &SendOptions,
) -> Result<Message, InjectError> {
    use droppen_provider::chat_attachments::{
        read_chat_image, read_chat_text_file, ChatAttachmentError, MAX_IMAGE_TOTAL_BYTES,
    };
    if options.images.is_empty() && options.files.is_empty() {
        let delivery = options.chat.then(|| DeliveryRecord {
            text: text.to_string(),
            images: Vec::new(),
            files: Vec::new(),
        });
        return build_message(text, text, &[], delivery, options);
    }

    if options.chat {
        let mut valid_images = Vec::with_capacity(options.images.len());
        let mut image_total = 0u64;
        for path in &options.images {
            let image = read_chat_image(paths, path)
                .await
                .map_err(|error| InjectError::message(error.to_string()))?;
            image_total = image_total.saturating_add(image.raw_len);
            if image_total > MAX_IMAGE_TOTAL_BYTES {
                return Err(InjectError::message(
                    ChatAttachmentError::ImagesTooLarge {
                        limit: MAX_IMAGE_TOTAL_BYTES,
                    }
                    .to_string(),
                ));
            }
            valid_images.push(path.clone());
        }
        let mut inlined = String::new();
        let mut text_total = 0u64;
        for path in &options.files {
            let content = read_chat_text_file(paths, path, &mut text_total)
                .await
                .map_err(|error| InjectError::message(error.to_string()))?;
            inlined.push_str(&droppen_sessions::util::chat_file_block(path, &content));
        }
        let wire_text = if inlined.is_empty() {
            text.to_string()
        } else {
            format!("{text}{inlined}")
        };
        let delivery = Some(DeliveryRecord {
            text: wire_text.clone(),
            images: valid_images.clone(),
            files: options.files.clone(),
        });
        return build_message(text, &wire_text, &valid_images, delivery, options);
    }

    let mut valid_images = Vec::new();
    let mut fallback_files = Vec::new();
    let mut image_total = 0u64;
    for path in &options.images {
        match read_chat_image(paths, path).await {
            Ok(image) if image_total.saturating_add(image.raw_len) <= MAX_IMAGE_TOTAL_BYTES => {
                image_total = image_total.saturating_add(image.raw_len);
                valid_images.push(path.clone());
            }
            Ok(_) => fallback_files.push(path.clone()),
            Err(error @ (ChatAttachmentError::OutsideUploads | ChatAttachmentError::HardLinked { .. })) => {
                return Err(InjectError::message(error.to_string()));
            }
            Err(_) => fallback_files.push(path.clone()),
        }
    }
    fallback_files.extend(options.files.iter().cloned());
    let wire_text = with_codex_files(text, &fallback_files);
    let delivery = (!valid_images.is_empty()).then(|| DeliveryRecord {
        text: wire_text.clone(),
        images: valid_images.clone(),
        files: Vec::new(),
    });
    build_message(text, &wire_text, &valid_images, delivery, options)
}

fn build_message(
    display_text: &str,
    wire_text: &str,
    images: &[String],
    delivery: Option<DeliveryRecord>,
    options: &SendOptions,
) -> Result<Message, InjectError> {
    let mut input = Vec::with_capacity(1 + images.len());
    if !wire_text.is_empty() || images.is_empty() {
        input.push(UserInput::Text {
            text: wire_text.to_string(),
        });
    }
    input.extend(images.iter().map(|path| UserInput::LocalImage { path }));
    let permission = if options.chat {
        Some("read-only")
    } else {
        options.permission.as_deref()
    };
    let sandbox_policy = match permission {
        Some(permission) => {
            codex_permission_flags(permission)?;
            Some(match permission {
                "auto" | "workspace-write" => SandboxPolicy::WorkspaceWrite,
                "read-only" => SandboxPolicy::ReadOnly,
                _ => SandboxPolicy::DangerFullAccess,
            })
        }
        None => None,
    };
    let params = TurnStartParams {
        input,
        approval_policy: ApprovalPolicy::Never,
        model: options.model.as_deref(),
        effort: options.effort.as_deref(),
        sandbox_policy,
    };
    Ok(Message {
        // The send path's choice when a client named the message, so its
        // receipt can follow it; see [`droppen_provider::receipts`].
        id: options.message_id.clone().unwrap_or_else(new_uuid),
        text: display_text.into(),
        // A stored value, because the journal reads back rows other builds
        // wrote.
        params: serde_json::to_value(params).expect("turn/start params encode"),
        request: None,
        uncertain: false,
        delivery,
    })
}

pub(super) async fn write(link: &mut shim::Link, message: &impl Serialize) -> Result<(), InjectError> {
    let line = serde_json::to_string(message).expect("Codex App Server message encode");
    tokio::time::timeout(RPC_TIMEOUT, link.send_line(&line))
        .await
        .map_err(|_| InjectError::message("Codex App Server write timed out"))?
        .map_err(|e| InjectError::Internal(e.to_string()))
}

// Headless permissions remain `never`. Unexpected interactive requests must get
// an explicit error, not hang the CLI or silently approve a tool invocation.
pub(super) async fn reject_request(link: &mut shim::Link, value: &Value) -> Result<(), InjectError> {
    let answer = ErrorResponse {
        id: &value["id"],
        error: RpcError {
            code: -32601,
            message: "interactive requests are not supported by Droppen",
        },
    };
    write(link, &answer).await
}

pub(super) async fn rpc(runs: &CodexRuns, link: &mut shim::Link, method: &str, params: impl Serialize) -> Result<Value, InjectError> {
    let id = new_uuid();
    write(link, &Request { id: &id, method, params }).await?;
    tokio::time::timeout(RPC_TIMEOUT, async {
        while let Some(output) = link.recv().await {
            let shim::Output::Line(line) = output else {
                return Err(InjectError::message("Codex handshake output was lost"));
            };
            let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
            if value.get("method").is_some() && value.get("id").is_some() {
                reject_request(link, &value).await?;
            }
            if value["id"] == id {
                if let Some(error) = value.get("error") {
                    return Err(InjectError::message(runs.host.redact(&error.to_string())));
                }
                return Ok(value["result"].clone());
            }
        }
        Err(InjectError::message("Codex App Server disconnected during handshake"))
    })
    .await
    .map_err(|_| InjectError::message("Codex App Server handshake timed out"))?
}
