//! Codex's model list: the cache the CLI itself writes, `models_cache.json`.

use std::path::Path;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde_json::Value;

use droppen_provider::models::{ModelInfo, ModelList, ModelListSource, Models, ModelsQuery};
use droppen_http::openapi_ext::Nullable;
use droppen_instance::paths::Paths;

/// What the Codex list falls back to when its cache is missing or unusable.
fn codex_fallback() -> Vec<ModelInfo> {
    ["gpt-5.6-sol", "gpt-5.5", "gpt-5.4"]
        .into_iter()
        .map(|id| ModelInfo {
            id: id.to_string(),
            label: Nullable(None),
            efforts: Nullable(None),
            default_effort: Nullable(None),
        })
        .collect()
}

/// Parse a JSON file, or `None` when it is missing or malformed.
async fn read_json(path: &Path) -> Option<Value> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    serde_json::from_str(&text).ok()
}

/// The listable entries of Codex's `models_cache.json`.
///
/// `visibility` gates entries the backend offers but does not want in a picker,
/// so anything other than `"list"` is dropped.
fn parse_codex_cache(raw: &Value) -> Vec<ModelInfo> {
    let Some(models) = raw.get("models").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in models {
        let Some(entry) = entry.as_object() else { continue };
        let Some(slug) = entry.get("slug").and_then(Value::as_str) else {
            continue;
        };
        if entry.get("visibility").and_then(Value::as_str) != Some("list") {
            continue;
        }
        // An absent key means "unknown", which is null; a present but junk-filled
        // array just yields the levels that did parse.
        let efforts = entry.get("supported_reasoning_levels").and_then(Value::as_array).map(|levels| {
            levels
                .iter()
                .filter_map(|level| level.as_object()?.get("effort")?.as_str().map(str::to_owned))
                .collect()
        });
        out.push(ModelInfo {
            id: slug.to_string(),
            label: Nullable(entry.get("display_name").and_then(Value::as_str).map(str::to_owned)),
            efforts: Nullable(efforts),
            default_effort: Nullable(entry.get("default_reasoning_level").and_then(Value::as_str).map(str::to_owned)),
        });
    }
    out
}

/// Codex's own cache of what its backend offers, or the static fallback.
pub async fn codex_models(cache: &Path) -> (Vec<ModelInfo>, ModelListSource) {
    let models = read_json(cache).await.map(|raw| parse_codex_cache(&raw)).unwrap_or_default();
    if models.is_empty() {
        (codex_fallback(), ModelListSource::Fallback)
    } else {
        (models, ModelListSource::Cache)
    }
}

/// Codex's models, for [`Models`]: its cache, read for every login alike,
/// since Codex has one config root.
pub(crate) struct CodexModels {
    /// The instance's paths, which say where Codex's home is.
    paths: Arc<Paths>,
}

impl CodexModels {
    /// The models of the Codex under `paths`.
    pub(crate) fn new(paths: Arc<Paths>) -> Self {
        Self { paths }
    }

    /// What Codex's own cache lists, or the static fallback.
    async fn cached(&self) -> (Vec<ModelInfo>, ModelListSource) {
        codex_models(&crate::sessions::codex::CodexHome::of(&self.paths).models_cache()).await
    }
}

impl Models for CodexModels {
    fn list<'a>(&'a self, _query: &'a ModelsQuery) -> BoxFuture<'a, Result<ModelList, String>> {
        Box::pin(async move {
            let (models, source) = self.cached().await;
            Ok(ModelList {
                models,
                source,
                hidden: None.into(),
            })
        })
    }

    fn ids<'a>(&'a self, _account_id: Option<&'a str>) -> BoxFuture<'a, Vec<String>> {
        Box::pin(async move {
            let (models, _) = self.cached().await;
            models.into_iter().map(|model| model.id).collect()
        })
    }

    // Codex publishes nothing static. Its rungs come out of the cache the CLI
    // itself writes, which is the *only* authority — so when that cache is
    // missing, this answers `None` and the caller accepts whatever it was given.
    fn accepted_efforts<'a>(&'a self, model: Option<&'a str>) -> BoxFuture<'a, Option<Vec<String>>> {
        Box::pin(async move {
            let (models, list) = self.cached().await;
            if list == ModelListSource::Fallback {
                return None;
            }
            // A named model answers for itself; without one, anything any listed
            // model accepts is accepted, because the model may be chosen later
            // (or left to the CLI's own default).
            let named = model.and_then(|wanted| models.iter().find(|entry| entry.id == wanted));
            let efforts: Vec<String> = match named {
                Some(entry) => entry.efforts.as_ref()?.clone(),
                None => {
                    let mut all: Vec<String> = models.iter().filter_map(|entry| entry.efforts.as_ref()).flatten().cloned().collect();
                    all.sort();
                    all.dedup();
                    all
                }
            };
            (!efforts.is_empty()).then_some(efforts)
        })
    }

    // The CLI's own modes, asked of [`crate::app_server::codex_permission_flags`] so
    // the two cannot drift: that function is what turns the value into `argv`,
    // and it already refuses what it does not know.
    fn permission_is_known(&self, permission: &str) -> bool {
        crate::app_server::codex_permission_flags(permission).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_codex_cache_keeps_only_listable_entries() {
        let models = parse_codex_cache(&json!({
            "models": [
                {
                    "slug": "gpt-5.6-sol",
                    "display_name": "GPT-5.6 Sol",
                    "visibility": "list",
                    "supported_reasoning_levels": [{ "effort": "low" }, { "effort": "high" }, { "note": "no effort here" }],
                    "default_reasoning_level": "high"
                },
                { "slug": "gpt-internal", "visibility": "hidden" },
                { "slug": "gpt-5.5", "visibility": "list" },
                { "visibility": "list" },
                "not an object"
            ]
        }));

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-5.6-sol");
        assert_eq!(models[0].label, Nullable(Some("GPT-5.6 Sol".to_string())));
        assert_eq!(models[0].efforts, Nullable(Some(vec!["low".to_string(), "high".to_string()])));
        assert_eq!(models[0].default_effort, Nullable(Some("high".to_string())));

        assert_eq!(models[1].id, "gpt-5.5");
        assert_eq!(models[1].label, Nullable(None), "an absent display_name is null, not missing");
        assert_eq!(models[1].efforts, Nullable(None), "an absent levels key is null, not an empty list");
    }

    #[test]
    fn a_cache_with_nothing_listable_parses_to_nothing() {
        assert!(parse_codex_cache(&json!({ "models": [] })).is_empty());
        assert!(parse_codex_cache(&json!({})).is_empty(), "a cache with no models key is empty");
    }

    #[tokio::test]
    async fn a_missing_codex_cache_falls_back_to_the_static_list() {
        let (models, source) = codex_models(Path::new("/nonexistent/models_cache.json")).await;
        assert_eq!(models, codex_fallback());
        assert_eq!(models[0].id, "gpt-5.6-sol");
        assert_eq!(source, ModelListSource::Fallback, "and says so");
    }
}
