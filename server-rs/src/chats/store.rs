//! The API-backed chat store, and the turn that streams a completion into it.
//!
//! Port of the "API-backed chat store" half of the former TypeScript server's
//! `chats.ts`. A transcript is one `<uuid>.json` under
//! `~/.local/share/remote-hand/api-chats`, written with the same keys in the
//! same order as the TypeScript store so the two servers can read each other's
//! chats.
//!
//! [`Chats::run_api_chat_turn`] is the only part that talks to the network. It
//! is written as "persist the user turn, stream, persist the assistant turn" in
//! that order deliberately: the user's message survives a provider outage, a
//! disconnect, or a restart, because it is on disk before the request is made.

use std::path::PathBuf;

use futures_util::StreamExt;
use gotcha::Schematic;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{ChatError, Chats};
use crate::sessions::{iso8601, Paths};

/// How long a completion may take.
///
/// `AbortSignal.timeout(300_000)` in `runApiChatTurn`.
const TURN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// How much of a failing provider's body is quoted back to the user.
const DETAIL_CAP: usize = 300;

/// How many characters of the first message become the chat's title.
const TITLE_CAP: usize = 60;

/// The title a chat carries until its first message names it.
const UNTITLED: &str = "new chat";

/// Who said something.
///
/// Spelled `ChatRole` rather than `Role` because `Schematic::name()` is the
/// Rust identifier and the schema registry has one component per name: a second
/// `Role` would silently merge with [`crate::sessions::util::Role`], whose
/// values are `user | assistant | system`. The contract inlines this enum, so
/// the component name itself is free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase", from = "String")]
pub enum ChatRole {
    /// The person.
    User,
    /// The provider.
    #[default]
    Assistant,
}

/// A role this server does not know is read as the assistant's, not as a parse
/// failure.
///
/// The strict enum made an unknown `role` an error, `read_api_chat` turned that
/// error into `None`, and every caller turned `None` into `chat not found` — so
/// one hand-edited message hid a whole transcript. The TypeScript cast the
/// parsed JSON and handed the string straight to the web, which renders
/// anything that is not `user` as the assistant; this lands in the same place
/// while keeping the wire enum the contract documents.
impl From<String> for ChatRole {
    fn from(raw: String) -> Self {
        match raw.as_str() {
            "user" => Self::User,
            _ => Self::Assistant,
        }
    }
}

/// One turn of an API-backed chat.
///
/// Every field defaults, for the same reason [`ApiChat`]'s do: the TypeScript
/// validated nothing below `id` and `messages`, so a turn missing its timestamp
/// still has to read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(default)]
pub struct ApiChatMessage {
    /// Who said it.
    pub role: ChatRole,
    /// What was said.
    pub content: String,
    /// When, ISO-8601.
    pub at: String,
}

/// A whole API-backed chat, exactly as it is stored.
///
/// `#[serde(default)]` keeps a hand-edited or half-written file readable: the
/// TypeScript casts the parsed JSON to `ApiChat` and checks only `id` and
/// `messages`, so a file missing `title` degrades rather than disappearing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(default, rename_all = "camelCase")]
pub struct ApiChat {
    /// Stable uuid; also the file name.
    pub id: String,
    /// Display name, seeded from the first message.
    pub title: String,
    /// The model this chat is answered by.
    pub model: String,
    /// When it was created, ISO-8601.
    pub created_at: String,
    /// When it last changed, ISO-8601.
    pub updated_at: String,
    /// Every turn, oldest first.
    pub messages: Vec<ApiChatMessage>,
}

/// Where the transcripts live.
///
/// `~/.local/share/remote-hand/api-chats` — [`Paths::data_dir`] is the
/// overridable spelling of the TypeScript's `join(homedir(), ".local", "share", …)`.
pub fn api_chats_dir(paths: &Paths) -> PathBuf {
    paths.data_dir().join("api-chats")
}

/// The file a chat id names, if the id can name one at all.
///
/// [`crate::sessions::reference::is_path_segment_id`] is the boundary that keeps
/// a client-supplied id from walking out of the directory: `..` and `/` are both
/// rejected, so the join below can only ever land inside `api-chats`.
pub fn chat_path(paths: &Paths, id: &str) -> Result<PathBuf, ChatError> {
    if !crate::sessions::reference::is_path_segment_id(id) {
        return Err(ChatError::message("invalid chat id"));
    }
    Ok(api_chats_dir(paths).join(format!("{id}.json")))
}

/// Now, in the format every other timestamp in the contract uses.
fn now() -> String {
    iso8601(std::time::SystemTime::now())
}

/// Read one chat, or `None` for anything unusable.
///
/// An unreadable file, an unusable id and a document that is not a chat all
/// collapse to `None`, which every caller turns into `chat not found`.
pub async fn read_api_chat(paths: &Paths, id: &str) -> Option<ApiChat> {
    let path = chat_path(paths, id).ok()?;
    let text = tokio::fs::read_to_string(path).await.ok()?;
    let raw = serde_json::from_str::<Value>(&text).ok()?;
    // The two fields the TypeScript checks before trusting the file.
    if !raw.get("id").is_some_and(Value::is_string) || !raw.get("messages").is_some_and(Value::is_array) {
        return None;
    }
    serde_json::from_value(raw).ok()
}

/// Persist a chat.
///
/// The body is `JSON.stringify(chat, null, 2) + "\n"`, and the tmp+rename swap
/// is what keeps a cancelled turn (the client hung up mid-stream) from leaving
/// half a document behind — which would read as a chat that vanished. The
/// temporary name does not end in `.json`, so a crash-leftover is invisible to
/// [`list_api_chats`].
async fn write_api_chat(paths: &Paths, chat: &ApiChat) -> std::io::Result<()> {
    let path = chat_path(paths, &chat.id).map_err(std::io::Error::other)?;
    tokio::fs::create_dir_all(api_chats_dir(paths)).await?;
    let body = format!("{}\n", serde_json::to_string_pretty(chat)?);
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, body).await?;
    tokio::fs::rename(&tmp, &path).await
}

/// Every stored chat, newest first.
///
/// A missing directory is an empty list, not an error: nothing has been created
/// yet is the normal state of a fresh install.
pub async fn list_api_chats(paths: &Paths) -> Vec<ApiChat> {
    let Ok(mut entries) = tokio::fs::read_dir(api_chats_dir(paths)).await else {
        return vec![];
    };
    let mut ids = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(id) = name.strip_suffix(".json") {
            ids.push(id.to_string());
        }
    }
    let mut chats = Vec::new();
    for id in ids {
        if let Some(chat) = read_api_chat(paths, &id).await {
            chats.push(chat);
        }
    }
    // `localeCompare` on ISO-8601 timestamps is a plain lexicographic compare.
    chats.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    chats
}

/// Where a completion's content chunks go as they arrive.
///
/// A trait rather than a channel so the store stays unaware of how the turn is
/// delivered; the route implements it over the SSE stream's sender, and the
/// tests over a plain vector.
pub trait DeltaSink {
    /// Hand over one chunk. `false` means the consumer is gone and the turn
    /// should stop where it is.
    fn delta(&self, text: String) -> impl std::future::Future<Output = bool> + Send;
}

impl Chats {
    /// Create an empty chat, resolving the model the same way the composer does.
    pub async fn create_api_chat(&self, model: Option<&str>) -> Result<ApiChat, ChatError> {
        let provider = self.require_provider().await?;
        let requested = model.unwrap_or_default().trim();
        let resolved = if requested.is_empty() { provider.model } else { requested.to_string() };
        if resolved.is_empty() {
            return Err(ChatError::message("no model configured — set a default model in settings"));
        }
        let stamp = now();
        let chat = ApiChat {
            id: uuid::Uuid::new_v4().to_string(),
            title: UNTITLED.to_string(),
            model: resolved,
            created_at: stamp.clone(),
            updated_at: stamp,
            messages: vec![],
        };
        let _guard = self.lock.lock().await;
        write_api_chat(&self.paths, &chat)
            .await
            .map_err(|error| ChatError::other(format!("cannot write the chat: {error}")))?;
        Ok(chat)
    }

    /// Delete a chat. `false` means there was nothing to delete.
    pub async fn delete_api_chat(&self, id: &str) -> bool {
        if read_api_chat(&self.paths, id).await.is_none() {
            return false;
        }
        let Ok(path) = chat_path(&self.paths, id) else { return false };
        let _ = tokio::fs::remove_file(path).await;
        true
    }

    /// Append the user turn, stream the completion, persist the assistant turn.
    ///
    /// The full reply is returned so the route can put it in the closing frame,
    /// which is how the web replaces its optimistically rendered deltas with the
    /// text that was actually stored.
    pub async fn run_api_chat_turn(&self, id: &str, text: &str, model: Option<&str>, sink: &(impl DeltaSink + Sync)) -> Result<String, ChatError> {
        let provider = self.require_provider().await?;
        let mut chat = read_api_chat(&self.paths, id).await.ok_or_else(|| ChatError::message("chat not found"))?;

        let stamp = now();
        chat.messages.push(ApiChatMessage {
            role: ChatRole::User,
            content: text.to_string(),
            at: stamp.clone(),
        });
        if chat.title == UNTITLED {
            chat.title = text.chars().take(TITLE_CAP).collect();
        }
        if let Some(model) = model.map(str::trim).filter(|m| !m.is_empty()) {
            chat.model = model.to_string();
        }
        chat.updated_at = stamp;
        self.persist(&chat).await?;

        let content = self.stream_completion(&provider, &chat, sink).await?;

        chat.messages.push(ApiChatMessage {
            role: ChatRole::Assistant,
            content: content.clone(),
            at: now(),
        });
        chat.updated_at = now();
        self.persist(&chat).await?;
        Ok(content)
    }

    async fn persist(&self, chat: &ApiChat) -> Result<(), ChatError> {
        let _guard = self.lock.lock().await;
        write_api_chat(&self.paths, chat)
            .await
            .map_err(|error| ChatError::other(format!("cannot write the chat: {error}")))
    }

    /// POST the conversation and reassemble the provider's `data:` frames.
    async fn stream_completion(
        &self,
        provider: &super::provider::StoredProvider,
        chat: &ApiChat,
        sink: &(impl DeltaSink + Sync),
    ) -> Result<String, ChatError> {
        let body = json!({
            "model": chat.model,
            "stream": true,
            "messages": chat.messages.iter().map(|m| json!({ "role": m.role, "content": m.content })).collect::<Vec<_>>(),
        });
        let response = self
            .http
            .post(format!("{}/chat/completions", provider.base_url))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {}", provider.api_key))
            .timeout(TURN_TIMEOUT)
            .json(&body)
            .send()
            .await
            .map_err(|error| ChatError::other(format!("provider request failed: {error}")))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let detail: String = response.text().await.unwrap_or_default().chars().take(DETAIL_CAP).collect();
            let suffix = if detail.is_empty() { String::new() } else { format!(" — {detail}") };
            return Err(ChatError::message(format!("provider request failed: HTTP {status}{suffix}")));
        }

        let mut stream = response.bytes_stream();
        // Bytes rather than a `String`, because a chunk boundary can fall in the
        // middle of a multi-byte character. Only the part up to the last newline
        // is decoded, and a newline is never inside one — which is the same
        // guarantee `TextDecoder({ stream: true })` gives the TypeScript.
        let mut buffer: Vec<u8> = Vec::new();
        let mut full = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| ChatError::other(format!("provider stream failed: {error}")))?;
            buffer.extend_from_slice(&chunk);
            let split = match buffer.iter().rposition(|&byte| byte == b'\n') {
                Some(index) => index + 1,
                None => continue,
            };
            let lines: Vec<String> = String::from_utf8_lossy(&buffer[..split]).lines().map(str::to_owned).collect();
            buffer.drain(..split);
            for line in lines {
                let Some(delta) = content_delta(&line) else { continue };
                full.push_str(&delta);
                if !sink.delta(delta).await {
                    return Err(ChatError::other("the client stopped reading the stream"));
                }
            }
        }
        Ok(full)
    }
}

/// The content chunk one provider line carries, if it carries one.
///
/// Anything else — a keep-alive comment, `[DONE]`, a frame with no content, a
/// line that is not JSON at all — is skipped rather than failing the turn,
/// because providers differ in what they interleave with the content.
fn content_delta(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let payload = trimmed.strip_prefix("data:")?.trim();
    if payload == "[DONE]" {
        return None;
    }
    let parsed = serde_json::from_str::<Value>(payload).ok()?;
    let delta = parsed.get("choices")?.get(0)?.get("delta")?.get("content")?.as_str()?;
    (!delta.is_empty()).then(|| delta.to_string())
}

#[cfg(test)]
mod tests_lenient {
    use super::*;

    /// The TypeScript cast the parsed JSON and checked only `id` and `messages`.
    /// A strict enum made an unknown `role` a parse failure, which
    /// [`read_api_chat`] turned into `None` and every caller into
    /// `chat not found` — so one odd message hid a whole transcript.
    #[tokio::test]
    async fn an_unknown_role_or_a_missing_timestamp_still_reads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        std::fs::create_dir_all(api_chats_dir(&paths)).expect("chats dir");
        std::fs::write(
            chat_path(&paths, "c1").expect("path"),
            r#"{"id":"c1","title":"t","model":"m","createdAt":"a","updatedAt":"b","messages":[
                {"role":"system","content":"you are helpful","at":"2026-01-01T00:00:00.000Z"},
                {"role":"user","content":"hi"}
            ]}"#,
        )
        .expect("seed");

        let chat = read_api_chat(&paths, "c1").await.expect("the chat must still read");
        assert_eq!(chat.messages.len(), 2);
        assert_eq!(
            chat.messages[0].role,
            ChatRole::Assistant,
            "the web renders anything that is not `user` as the assistant"
        );
        assert_eq!(chat.messages[1].role, ChatRole::User);
        assert_eq!(chat.messages[1].at, "", "a missing timestamp is empty, not a refusal");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A sink that keeps every chunk, for asserting on what a turn emitted.
    struct Collected(Arc<Mutex<Vec<String>>>);

    impl DeltaSink for Collected {
        async fn delta(&self, text: String) -> bool {
            self.0.lock().expect("sink poisoned").push(text);
            true
        }
    }

    fn paths() -> (tempfile::TempDir, Paths) {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        (home, paths)
    }

    #[test]
    fn only_a_word_character_id_names_a_file() {
        let (_home, paths) = paths();
        let dir = api_chats_dir(&paths);
        for id in ["abc", "a-b_c", "11111111-2222-3333-4444-555555555555"] {
            assert_eq!(chat_path(&paths, id).expect("usable"), dir.join(format!("{id}.json")));
        }
        for id in ["", "..", "../../etc/passwd", "a/b", "a.json", "a b"] {
            assert_eq!(chat_path(&paths, id).expect_err("rejected").to_string(), "invalid chat id");
        }
    }

    #[tokio::test]
    async fn a_chat_round_trips_in_the_typescript_shape() {
        let (_home, paths) = paths();
        let chat = ApiChat {
            id: "abc".into(),
            title: "hello".into(),
            model: "gpt-4o".into(),
            created_at: "2024-05-01T10:00:00.000Z".into(),
            updated_at: "2024-05-01T10:00:01.000Z".into(),
            messages: vec![ApiChatMessage {
                role: ChatRole::User,
                content: "hi".into(),
                at: "2024-05-01T10:00:00.000Z".into(),
            }],
        };
        write_api_chat(&paths, &chat).await.expect("write");

        let raw = std::fs::read_to_string(chat_path(&paths, "abc").expect("path")).expect("read");
        assert!(raw.ends_with("}\n"), "the TypeScript appends a trailing newline");
        assert_eq!(
            serde_json::from_str::<Value>(&raw).expect("json"),
            json!({
                "id": "abc",
                "title": "hello",
                "model": "gpt-4o",
                "createdAt": "2024-05-01T10:00:00.000Z",
                "updatedAt": "2024-05-01T10:00:01.000Z",
                "messages": [{ "role": "user", "content": "hi", "at": "2024-05-01T10:00:00.000Z" }],
            })
        );
        assert_eq!(read_api_chat(&paths, "abc").await.expect("read back"), chat);
    }

    #[tokio::test]
    async fn an_unusable_file_is_not_a_chat() {
        let (_home, paths) = paths();
        std::fs::create_dir_all(api_chats_dir(&paths)).expect("dir");
        let path = api_chats_dir(&paths).join("abc.json");

        for body in ["half a docu", "[]", r#"{"id": 7, "messages": []}"#, r#"{"id": "abc"}"#] {
            std::fs::write(&path, body).expect("seed");
            assert!(read_api_chat(&paths, "abc").await.is_none(), "{body} should not read as a chat");
        }
        // A chat missing everything but the two checked fields still reads.
        std::fs::write(&path, r#"{"id": "abc", "messages": []}"#).expect("seed");
        let chat = read_api_chat(&paths, "abc").await.expect("degraded but readable");
        assert_eq!(chat.title, "");
        assert!(chat.messages.is_empty());
    }

    #[tokio::test]
    async fn the_list_is_newest_first_and_ignores_everything_else() {
        let (_home, paths) = paths();
        std::fs::create_dir_all(api_chats_dir(&paths)).expect("dir");
        for (id, updated) in [("older", "2024-05-01T10:00:00.000Z"), ("newer", "2024-06-01T10:00:00.000Z")] {
            let chat = ApiChat {
                id: id.into(),
                updated_at: updated.into(),
                ..Default::default()
            };
            write_api_chat(&paths, &chat).await.expect("write");
        }
        std::fs::write(api_chats_dir(&paths).join("stray.json.tmp"), "{}").expect("leftover");
        std::fs::write(api_chats_dir(&paths).join("notes.txt"), "hello").expect("stray");

        let ids: Vec<String> = list_api_chats(&paths).await.into_iter().map(|c| c.id).collect();
        assert_eq!(ids, vec!["newer".to_string(), "older".to_string()]);
    }

    #[test]
    fn a_provider_line_yields_a_chunk_only_when_it_carries_one() {
        assert_eq!(
            content_delta(r#"data: {"choices":[{"delta":{"content":"hel"}}]}"#).as_deref(),
            Some("hel")
        );
        // Providers interleave all of these with the content.
        for line in [
            "",
            ": keep-alive",
            "data: [DONE]",
            "event: message",
            "data: not json",
            r#"data: {"choices":[]}"#,
            r#"data: {"choices":[{"delta":{}}]}"#,
            r#"data: {"choices":[{"delta":{"content":""}}]}"#,
            r#"data: {"choices":[{"delta":{"content":42}}]}"#,
        ] {
            assert_eq!(content_delta(line), None, "{line} carries no content");
        }
    }

    #[tokio::test]
    async fn the_sink_sees_every_chunk_in_order() {
        let seen = Arc::new(Mutex::new(vec![]));
        let sink = Collected(seen.clone());
        for chunk in ["a", "b", "c"] {
            assert!(sink.delta(chunk.to_string()).await);
        }
        assert_eq!(*seen.lock().expect("sink"), vec!["a".to_string(), "b".into(), "c".into()]);
    }
}
