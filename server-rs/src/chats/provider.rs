//! The BYO-API provider configuration: a base URL, an API key and a default
//! model.
//!
//! Port of the "OpenAI-compatible provider configuration" half of
//! the former TypeScript server's `chats.ts`. Three rows in the machine-wide
//! state database ([`crate::db`]) under [`scope::CHAT_PROVIDER`], keyed by the
//! wire's own `baseUrl`, `apiKey` and `model`. Three properties are
//! load-bearing and are what the tests in this module pin down:
//!
//! * **the key never leaves the server.** Everything the API returns goes
//!   through [`ChatProviderInfo`], which carries `hasKey: bool` and no key. The
//!   raw value is read by exactly two places — the `Authorization` header of the
//!   `/models` call and of the completion call — and by nothing else.
//! * **the key is never half-written.** `chat-provider.json` was created with
//!   `O_TRUNC` and written in place, which is the one shape that can leave a
//!   *truncated* file behind: a crash between the truncate and the write, and
//!   the key is simply gone, silently, with the settings panel showing "no key
//!   configured" and no way to know it was ever there. The three rows land in
//!   one transaction, which has no such state.
//! * **`chat-provider.json` is read once and never written.** The import drains
//!   it on the first boot after the upgrade and nothing touches it again. It was
//!   a mirror for one version, kept `0600` and byte-compatible with the
//!   TypeScript store so that a `.prev` binary which had never heard of the
//!   database would still find the key; the rollback floor has since risen to a
//!   build that reads the database, and a second copy of an API key is not a
//!   thing to keep on disk for a reason that no longer exists. The remaining
//!   copy is the database, which is `0600` for exactly the reason the file was;
//!   see [`crate::db::Db`]. The file itself is left where it is rather than
//!   deleted — it is inert, and it is the cheapest way to seed a database that
//!   has to be rebuilt from scratch.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gotcha::Schematic;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ChatError, Chats};
use crate::db::{scope, Db};
use crate::openapi_ext::Optional;
use crate::sessions::Paths;

/// How long the provider's `/models` call may take.
///
/// `AbortSignal.timeout(15_000)` in `providerModels`.
const MODELS_TIMEOUT: Duration = Duration::from_secs(15);

/// The stored configuration, key included. Never serialized to a client.
///
/// Field order is the contract: `JSON.stringify` writes keys in insertion order
/// and so does `serde_json`, so the two servers produce the same bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredProvider {
    /// The OpenAI-compatible endpoint root, without a trailing slash.
    pub base_url: String,
    /// The bearer token. The one field that must never reach a response.
    pub api_key: String,
    /// The model new chats default to.
    pub model: String,
}

/// What a client is allowed to learn about the provider.
///
/// Named `ChatProviderInfo` because that is the schema name in
/// `openapi/openapi.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ChatProviderInfo {
    /// The configured endpoint root, or `""`.
    pub base_url: String,
    /// The configured default model, or `""`.
    pub model: String,
    /// Whether a key is stored. The key itself is never sent.
    pub has_key: bool,
}

/// The body of `PUT /api/chat-provider`; every field is a patch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ChatProviderUpdate {
    /// New endpoint root; absent leaves it alone.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub base_url: Optional<String>,
    /// New key; an explicit empty string clears the stored one.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub api_key: Optional<String>,
    /// New default model; absent leaves it alone.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub model: Optional<String>,
}

/// The response of `GET /api/chat-provider/models`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct ChatModelList {
    /// Model ids the provider offers, sorted.
    pub models: Vec<String>,
}

/// Where the file the import drains lives.
///
/// `~/.config/remote-hand/chat-provider.json` — [`Paths::config_dir`] is the
/// overridable spelling of the TypeScript's `join(homedir(), ".config", …)`.
pub fn provider_file(paths: &Paths) -> PathBuf {
    paths.config_dir().join("chat-provider.json")
}

/// The database, with `chat-provider.json` drained into it if nobody has yet.
fn store(paths: &Paths) -> Arc<Db> {
    let db = crate::db::at(paths.config_db());
    db.import_once(scope::CHAT_PROVIDER, || {
        let stored = provider_from_value(&parse_file(paths));
        vec![
            ("baseUrl".to_string(), stored.base_url),
            ("apiKey".to_string(), stored.api_key),
            ("model".to_string(), stored.model),
        ]
    });
    db
}

/// The file, parsed. Read exactly once per install, by the import.
fn parse_file(paths: &Paths) -> Value {
    std::fs::read_to_string(provider_file(paths))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

/// The three fields out of arbitrary JSON, coercing anything unusable to `""`.
///
/// Split out from [`read_provider`] because this — not the I/O — is the part
/// worth pinning down, the split [`crate::prefs`] and [`crate::attention::seen`]
/// both make. Malformed input reads as "nothing configured" rather than as an
/// error: the settings panel has to stay usable so the user can fix it. The
/// per-field coercion mirrors the TypeScript's
/// `typeof raw.baseUrl === "string" ? raw.baseUrl : ""`, so a hand-edited file
/// with a number in it degrades the same way on both servers.
fn provider_from_value(raw: &Value) -> StoredProvider {
    let field = |key: &str| raw.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    StoredProvider {
        base_url: field("baseUrl"),
        api_key: field("apiKey"),
        model: field("model"),
    }
}

/// The stored configuration; an empty store reads as nothing configured.
pub async fn read_provider(paths: &Paths) -> StoredProvider {
    let rows = store(paths).kv_scope(scope::CHAT_PROVIDER);
    // Back through the same reader the file went through, so the rows and a
    // hand-edited file cannot come to mean different things.
    provider_from_value(&Value::Object(rows.into_iter().map(|(key, value)| (key, Value::String(value))).collect()))
}

impl StoredProvider {
    /// The redacted view, and the only thing a route may return.
    pub fn info(&self) -> ChatProviderInfo {
        ChatProviderInfo {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            has_key: !self.api_key.is_empty(),
        }
    }

    /// Apply a patch, normalising exactly as `updateProvider` does.
    fn patched(&self, patch: &ChatProviderUpdate) -> StoredProvider {
        StoredProvider {
            base_url: patch
                .base_url
                .as_deref()
                .unwrap_or(&self.base_url)
                .trim()
                .trim_end_matches('/')
                .to_string(),
            // An explicit empty string clears the stored key; an absent field
            // keeps it. That distinction is why the field is `Optional`.
            api_key: patch
                .api_key
                .as_deref()
                .map_or_else(|| self.api_key.clone(), |key| key.trim().to_string()),
            model: patch.model.as_deref().unwrap_or(&self.model).trim().to_string(),
        }
    }
}

impl Chats {
    /// What a client is told about the provider.
    pub async fn provider_info(&self) -> ChatProviderInfo {
        read_provider(&self.paths).await.info()
    }

    /// Merge a patch into the stored configuration and persist it.
    ///
    /// Serialized by the store lock so two concurrent settings saves cannot
    /// read the same "current" and each write their own half of it — the
    /// TypeScript's `withLock` around the read-modify-write.
    pub async fn update_provider(&self, patch: &ChatProviderUpdate) -> Result<ChatProviderInfo, ChatError> {
        let _guard = self.lock.lock().await;
        let next = read_provider(&self.paths).await.patched(patch);
        write_provider(&self.paths, &next)
            .await
            .map_err(|error| ChatError::other(format!("cannot write the provider configuration: {error}")))?;
        Ok(next.info())
    }

    /// The provider, or the refusal the settings panel shows.
    ///
    /// `requireProvider`. The two messages are the contract: the web surfaces
    /// them verbatim next to the fields the user has to fill in.
    pub async fn require_provider(&self) -> Result<StoredProvider, ChatError> {
        let provider = read_provider(&self.paths).await;
        if provider.base_url.is_empty() {
            return Err(ChatError::message("no API base URL configured — set it in settings"));
        }
        if provider.api_key.is_empty() {
            return Err(ChatError::message("no API key configured — set it in settings"));
        }
        Ok(provider)
    }

    /// Model ids from the provider's `/models` endpoint, sorted.
    pub async fn provider_models(&self) -> Result<Vec<String>, ChatError> {
        let provider = self.require_provider().await?;
        let response = self
            .http
            .get(format!("{}/models", provider.base_url))
            .header("authorization", format!("Bearer {}", provider.api_key))
            .timeout(MODELS_TIMEOUT)
            .send()
            .await
            // A transport failure is a plain `Error` in the TypeScript, which
            // the route replaces with its own wording rather than showing the
            // user a URL that may hold the key.
            .map_err(|error| ChatError::other(format!("provider /models request failed: {error}")))?;
        if !response.status().is_success() {
            return Err(ChatError::message(format!(
                "provider /models failed: HTTP {}",
                response.status().as_u16()
            )));
        }
        let body = response
            .json::<Value>()
            .await
            .map_err(|error| ChatError::other(format!("provider /models returned no JSON: {error}")))?;
        let Some(entries) = body.get("data").and_then(Value::as_array) else {
            return Ok(vec![]);
        };
        let mut models: Vec<String> = entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(Value::as_str))
            .map(str::to_owned)
            .collect();
        models.sort();
        Ok(models)
    }
}

/// Commit the three rows.
///
/// The transaction is what makes the key safe: what the next read finds is
/// either the whole of the old configuration or the whole of the new one, never
/// a half of either. Still `io::Result` because [`Chats::update_provider`] turns
/// its error into the message the settings panel shows; there is no I/O left in
/// here that can fail, so the result is unconditionally `Ok`.
async fn write_provider(paths: &Paths, provider: &StoredProvider) -> std::io::Result<()> {
    store(paths).kv_replace_scope(
        scope::CHAT_PROVIDER,
        [
            ("baseUrl", provider.base_url.as_str()),
            ("apiKey", provider.api_key.as_str()),
            ("model", provider.model.as_str()),
        ],
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chats() -> (tempfile::TempDir, std::sync::Arc<Chats>) {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = std::sync::Arc::new(Paths::new(home.path().to_path_buf()));
        (home, Chats::new(paths))
    }

    fn patch(base_url: Option<&str>, api_key: Option<&str>, model: Option<&str>) -> ChatProviderUpdate {
        ChatProviderUpdate {
            base_url: Optional(base_url.map(str::to_owned)),
            api_key: Optional(api_key.map(str::to_owned)),
            model: Optional(model.map(str::to_owned)),
        }
    }

    #[tokio::test]
    async fn the_key_is_stored_but_never_reported() {
        let (_home, chats) = chats();
        let info = chats
            .update_provider(&patch(Some("https://api.example.test/v1"), Some("sk-secret-value"), Some("gpt-4o")))
            .await
            .expect("update");

        assert!(info.has_key, "the client learns that a key exists");
        let json = serde_json::to_value(&info).expect("encode");
        assert_eq!(
            json,
            serde_json::json!({ "baseUrl": "https://api.example.test/v1", "model": "gpt-4o", "hasKey": true }),
            "the redacted view has exactly three fields and none of them is the key"
        );
        assert!(
            !serde_json::to_string(&json).expect("encode").contains("sk-secret-value"),
            "the key must not reach a response by any spelling"
        );
        // …and the same for the second way a client asks.
        assert_eq!(chats.provider_info().await, info);
        assert_eq!(chats.require_provider().await.expect("configured").api_key, "sk-secret-value");
    }

    /// The inverse of the test that used to check the mirror's bytes and its
    /// mode: there is no second copy of the key on disk to get either wrong.
    #[tokio::test]
    async fn saving_a_key_writes_no_file() {
        let (_home, chats) = chats();
        chats
            .update_provider(&patch(Some("https://api.example.test/v1"), Some("sk-secret-value"), Some("gpt-4o")))
            .await
            .expect("update");

        let path = provider_file(&chats.paths);
        assert!(!path.exists(), "no file is created where there was none");
        assert!(!path.with_extension("json.tmp").exists());
    }

    /// A file left over from before the upgrade is an input, once, and never an
    /// output — which is what makes a hand-restored `chat-provider.json` a safe
    /// thing to drop next to an empty database.
    #[tokio::test]
    async fn a_pre_existing_file_is_read_and_then_left_untouched() {
        let (_home, chats) = chats();
        let path = provider_file(&chats.paths);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("config dir");
        let before = "{\n  \"baseUrl\": \"https://api.example.test/v1\",\n  \"apiKey\": \"sk-from-the-file\",\n  \"model\": \"gpt-4o\"\n}\n";
        std::fs::write(&path, before).expect("seed");

        chats.update_provider(&patch(None, Some("sk-typed-since"), None)).await.expect("update");
        assert_eq!(read_provider(&chats.paths).await.api_key, "sk-typed-since");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            before,
            "byte for byte as it was found, import and re-key notwithstanding"
        );
    }

    #[tokio::test]
    async fn a_patch_normalises_and_only_an_empty_key_clears_the_key() {
        let (_home, chats) = chats();
        chats
            .update_provider(&patch(Some("  https://api.example.test/v1///  "), Some("  sk-1  "), Some("  gpt-4o  ")))
            .await
            .expect("update");
        let stored = read_provider(&chats.paths).await;
        assert_eq!(stored.base_url, "https://api.example.test/v1", "trailing slashes are stripped");
        assert_eq!(stored.api_key, "sk-1", "the key is trimmed");
        assert_eq!(stored.model, "gpt-4o");

        // A patch that names only the model leaves the key alone.
        chats.update_provider(&patch(None, None, Some("gpt-4o-mini"))).await.expect("update");
        assert_eq!(read_provider(&chats.paths).await.api_key, "sk-1", "an absent key keeps the stored one");

        // An explicit empty string is how the settings panel clears it.
        let info = chats.update_provider(&patch(None, Some(""), None)).await.expect("update");
        assert!(!info.has_key);
        assert_eq!(read_provider(&chats.paths).await.api_key, "");
    }

    #[tokio::test]
    async fn nothing_configured_reads_as_empty_and_refuses_by_field() {
        let (_home, chats) = chats();
        assert_eq!(
            chats.provider_info().await,
            ChatProviderInfo {
                base_url: String::new(),
                model: String::new(),
                has_key: false,
            }
        );
        assert_eq!(
            chats.require_provider().await.expect_err("no base url").to_string(),
            "no API base URL configured — set it in settings"
        );

        chats
            .update_provider(&patch(Some("https://api.example.test/v1"), None, None))
            .await
            .expect("update");
        assert_eq!(
            chats.require_provider().await.expect_err("no key").to_string(),
            "no API key configured — set it in settings"
        );
    }

    #[test]
    fn a_corrupt_or_wrongly_typed_document_reads_as_nothing_configured() {
        for body in ["not json at all", "[]", r#"{"baseUrl": 7, "apiKey": null}"#] {
            let raw = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
            assert_eq!(provider_from_value(&raw), StoredProvider::default(), "{body} should read as unconfigured");
        }
        // A field of the wrong type costs only itself, not the other two.
        let mixed = provider_from_value(&serde_json::json!({ "baseUrl": "https://api.example.test/v1", "apiKey": 7, "model": "gpt-4o" }));
        assert_eq!(mixed.base_url, "https://api.example.test/v1");
        assert_eq!(mixed.api_key, "");
        assert_eq!(mixed.model, "gpt-4o");
    }

    #[tokio::test]
    async fn a_corrupt_file_is_imported_as_nothing_configured() {
        let (_home, chats) = chats();
        let path = provider_file(&chats.paths);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("config dir");
        std::fs::write(&path, "not json at all").expect("seed");
        assert_eq!(read_provider(&chats.paths).await, StoredProvider::default());
    }

    /// The file is drained on the first look and never consulted again. If it
    /// were re-read, a key the user has since cleared would be handed back by
    /// the file that still held it — which for a secret is the wrong direction
    /// twice over.
    #[tokio::test]
    async fn the_file_is_imported_once_and_then_stops_being_the_truth() {
        let (_home, chats) = chats();
        let path = provider_file(&chats.paths);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("config dir");
        std::fs::write(
            &path,
            r#"{"baseUrl":"https://api.example.test/v1","apiKey":"sk-from-the-file","model":"gpt-4o"}"#,
        )
        .expect("seed");

        assert_eq!(
            read_provider(&chats.paths).await.api_key,
            "sk-from-the-file",
            "the key survives the upgrade"
        );

        std::fs::write(&path, r#"{"baseUrl":"https://elsewhere.test","apiKey":"sk-edited","model":"other"}"#).expect("re-seed");
        assert_eq!(
            read_provider(&chats.paths).await.api_key,
            "sk-from-the-file",
            "the import does not run a second time"
        );

        chats.update_provider(&patch(None, Some(""), None)).await.expect("clear");
        assert_eq!(read_provider(&chats.paths).await.api_key, "", "and a cleared key stays cleared");
    }

    #[tokio::test]
    async fn the_configuration_is_still_there_through_a_fresh_connection() {
        let (_home, chats) = chats();
        chats
            .update_provider(&patch(Some("https://api.example.test/v1"), Some("sk-secret-value"), Some("gpt-4o")))
            .await
            .expect("update");

        let reopened = crate::db::Db::open(chats.paths.config_db());
        let rows = reopened.kv_scope(scope::CHAT_PROVIDER);
        assert_eq!(rows.get("apiKey").map(String::as_str), Some("sk-secret-value"));
        assert_eq!(rows.get("baseUrl").map(String::as_str), Some("https://api.example.test/v1"));
        assert_eq!(rows.get("model").map(String::as_str), Some("gpt-4o"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(chats.paths.config_db()).expect("stat").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the database holds the key now, so it is as private as the file was");
        }
    }
}
