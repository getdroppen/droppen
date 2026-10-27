//! `/api/plugins/*`: the plugin switches and the document store, over HTTP.
//!
//! Registered once and generic — nothing here names a plugin — so the OpenAPI
//! document is the same whichever plugins a build bundles. Every
//! `/api/plugins/{id}/…` call goes through [`super::Plugins::require_enabled`]
//! first, so a plugin that is not on answers `404 plugin_disabled` and never
//! gets a store made for it. The contract is `docs/plugins/storage-protocol.md`.
//!
//! A document name is one path segment: a client sends `events/2026-10` as
//! `events%2F2026-10`, and the router hands the handler the decoded name.

use std::sync::Arc;

use gotcha::axum::body::Bytes;
use gotcha::axum::extract::{FromRequest, Request};
use gotcha::axum::http::StatusCode;
use gotcha::oas::{Parameter, RequestBody};
use gotcha::{api, Either, Json, ParameterProvider, State};
use serde::de::DeserializeOwned;
use serde_json::json;

use super::store::{StoreError, MAX_REQUEST_BYTES};
use super::wire::{
    PluginBatch, PluginBatchResult, PluginCurrentDoc, PluginDoc, PluginDocConflict, PluginDocHistory, PluginDocList, PluginDocQuery,
    PluginHistoryQuery, PluginList, PluginPutDoc, PluginStatus, PluginSwitch, PluginTooLarge,
};
use super::{DecisionReason, PluginError, Plugins};
use crate::api::error::ApiError;
use crate::api::extract::{ApiJson, ApiPath, ApiQuery};
use crate::api::{ApiRouter, AppState};
use crate::auth::MaybeIdentity;
use crate::openapi_ext::ExtraResponses;
use droppen_plugin_sdk::codes;

const LIST: &str = "/api/plugins";
const ENABLED: &str = "/api/plugins/{id}/enabled";
const DOCS: &str = "/api/plugins/{id}/docs";
const DOC: &str = "/api/plugins/{id}/docs/{name}";
const HISTORY: &str = "/api/plugins/{id}/docs/{name}/history";
const BATCH: &str = "/api/plugins/{id}/batch";

fn coded(status: StatusCode, code: &str, message: impl Into<String>) -> ApiError {
    ApiError::with_body(status, json!({ "error": code, "message": message.into() }))
}

impl From<PluginError> for ApiError {
    fn from(error: PluginError) -> Self {
        let message = error.to_string();
        match error {
            PluginError::NotFound(_) => coded(StatusCode::NOT_FOUND, codes::PLUGIN_NOT_FOUND, message),
            PluginError::Disabled { .. } => coded(StatusCode::NOT_FOUND, codes::PLUGIN_DISABLED, message),
            PluginError::Forced(_) => coded(StatusCode::CONFLICT, codes::FORCED_BY_CONFIG, message),
            PluginError::Store(store) => store.into(),
        }
    }
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        let message = error.to_string();
        match error {
            StoreError::Invalid(_) => coded(StatusCode::BAD_REQUEST, codes::INVALID, message),
            StoreError::DocNotFound(_) => coded(StatusCode::NOT_FOUND, codes::DOC_NOT_FOUND, message),
            StoreError::Conflict { doc, current } => ApiError::with_body(
                StatusCode::CONFLICT,
                serde_json::to_value(PluginDocConflict {
                    error: codes::CONFLICT.to_string(),
                    message,
                    doc,
                    current: current.map(PluginCurrentDoc::from).into(),
                })
                .unwrap_or_default(),
            ),
            StoreError::TooLarge { message, limit } => too_large(message, limit),
            StoreError::NotEmpty(_) => coded(StatusCode::CONFLICT, "not_empty", message),
            StoreError::Rejected(_) | StoreError::Failed(_) => {
                tracing::error!(%message, "a plugin store call failed");
                coded(StatusCode::INTERNAL_SERVER_ERROR, "failed", message)
            }
        }
    }
}

fn too_large(message: String, limit: u64) -> ApiError {
    ApiError::with_body(
        StatusCode::PAYLOAD_TOO_LARGE,
        serde_json::to_value(PluginTooLarge {
            error: codes::TOO_LARGE.to_string(),
            message,
            limit,
        })
        .unwrap_or_default(),
    )
}

/// A JSON body held to [`MAX_REQUEST_BYTES`], whose overflow is the store's own
/// `413 too_large` rather than axum's text, and whose bad JSON is
/// `400 invalid`.
///
/// The limit itself is the route's [`DefaultBodyLimit`](gotcha::axum::extract::DefaultBodyLimit),
/// which [`routes`] raises on these routes only.
pub struct PluginJson<T>(pub T);

impl<T, S> FromRequest<S> for PluginJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(request, state).await.map_err(|rejection| {
            if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
                too_large(format!("one request may carry {MAX_REQUEST_BYTES} bytes"), MAX_REQUEST_BYTES)
            } else {
                coded(StatusCode::BAD_REQUEST, codes::INVALID, rejection.body_text())
            }
        })?;
        serde_json::from_slice(&bytes)
            .map(PluginJson)
            .map_err(|error| coded(StatusCode::BAD_REQUEST, codes::INVALID, error.to_string()))
    }
}

impl<T> ParameterProvider for PluginJson<T>
where
    Json<T>: ParameterProvider,
{
    fn generate(url: String) -> Either<Vec<Parameter>, RequestBody> {
        <Json<T> as ParameterProvider>::generate(url)
    }
}

/// Run a blocking store call off the runtime: a write is an fsync.
async fn blocking<T: Send + 'static>(
    plugins: &Arc<Plugins>,
    work: impl FnOnce(&Plugins) -> Result<T, PluginError> + Send + 'static,
) -> Result<T, ApiError> {
    let plugins = plugins.clone();
    tokio::task::spawn_blocking(move || work(&plugins))
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?
        .map_err(ApiError::from)
}

fn caller(who: &MaybeIdentity) -> Option<String> {
    who.0.as_ref().map(|identity| identity.username.clone())
}

/// Every plugin this build bundles, as this instance sees it.
#[api(id = "list_plugins", group = "plugins", summary = "List plugins")]
pub async fn list_plugins(State(state): State<AppState>) -> Json<PluginList> {
    Json(PluginList {
        plugins: state.plugins.status_async().await.into_iter().map(PluginStatus::from).collect(),
    })
}

/// Turn a plugin on or off for this instance. Any signed-in user may.
#[api(id = "set_plugin_enabled", group = "plugins", summary = "Switch a plugin")]
pub async fn set_plugin_enabled(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    #[api(skip)] who: MaybeIdentity,
    ApiJson(body): ApiJson<PluginSwitch>,
) -> Result<Json<PluginStatus>, ApiError> {
    let by = caller(&who);
    blocking(&state.plugins, move |plugins| {
        plugins.set_enabled(&id, body.enabled, by.as_deref(), DecisionReason::Settings)
    })
    .await
    .map(|status| Json(status.into()))
}

/// A plugin's documents, without values, by name.
#[api(id = "list_plugin_docs", group = "plugins", summary = "List a plugin's documents")]
pub async fn list_plugin_docs(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<PluginDocQuery>,
) -> Result<Json<PluginDocList>, ApiError> {
    let prefix = query.prefix.unwrap_or_default();
    blocking(&state.plugins, move |plugins| plugins.list(&id, &prefix)).await.map(|docs| {
        Json(PluginDocList {
            docs: docs.into_iter().map(Into::into).collect(),
        })
    })
}

/// One document.
#[api(id = "get_plugin_doc", group = "plugins", summary = "Read a plugin document")]
pub async fn get_plugin_doc(State(state): State<AppState>, ApiPath((id, name)): ApiPath<(String, String)>) -> Result<Json<PluginDoc>, ApiError> {
    blocking(&state.plugins, move |plugins| plugins.get(&id, &name))
        .await
        .map(|doc| Json(doc.into()))
}

/// Write one document by compare-and-set.
#[api(id = "put_plugin_doc", group = "plugins", summary = "Write a plugin document")]
pub async fn put_plugin_doc(
    State(state): State<AppState>,
    ApiPath((id, name)): ApiPath<(String, String)>,
    #[api(skip)] who: MaybeIdentity,
    PluginJson(body): PluginJson<PluginPutDoc>,
) -> Result<Json<PluginDoc>, ApiError> {
    let by = caller(&who);
    blocking(&state.plugins, move |plugins| plugins.put(&id, &name, body.into(), by.as_deref()))
        .await
        .map(|doc| Json(doc.into()))
}

/// Write several documents, all or none.
#[api(id = "plugin_batch", group = "plugins", summary = "Write plugin documents together")]
pub async fn plugin_batch(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    #[api(skip)] who: MaybeIdentity,
    PluginJson(body): PluginJson<PluginBatch>,
) -> Result<Json<PluginBatchResult>, ApiError> {
    let by = caller(&who);
    let writes = body.writes.into_iter().map(Into::into).collect();
    blocking(&state.plugins, move |plugins| plugins.batch(&id, writes, by.as_deref()))
        .await
        .map(|docs| {
            Json(PluginBatchResult {
                docs: docs.into_iter().map(Into::into).collect(),
            })
        })
}

/// The versions a document was replaced from, newest first.
#[api(id = "plugin_doc_history", group = "plugins", summary = "A plugin document's history")]
pub async fn plugin_doc_history(
    State(state): State<AppState>,
    ApiPath((id, name)): ApiPath<(String, String)>,
    ApiQuery(query): ApiQuery<PluginHistoryQuery>,
) -> Result<Json<PluginDocHistory>, ApiError> {
    let offset = query.offset.unwrap_or(0) as usize;
    let limit = query.limit.map(|limit| limit as usize);
    blocking(&state.plugins, move |plugins| plugins.history(&id, &name, offset, limit))
        .await
        .map(|page| {
            Json(PluginDocHistory {
                versions: page.versions.into_iter().map(Into::into).collect(),
                more: page.more,
            })
        })
}

/// Register the routes.
///
/// The two that carry a document are a sub-router of their own, so the raised
/// body limit is theirs and no other route's (as `crate::files` does it). The
/// read of a single document shares its path, and so its sub-router; it has no
/// body for the limit to matter to.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router
        .get(LIST, list_plugins)
        .put(ENABLED, set_plugin_enabled)
        .get(DOCS, list_plugin_docs)
        .get(HISTORY, plugin_doc_history)
        .merge(
            ApiRouter::default()
                .get(DOC, get_plugin_doc)
                .put(DOC, put_plugin_doc)
                .post(BATCH, plugin_batch)
                .layer(gotcha::axum::extract::DefaultBodyLimit::max(MAX_REQUEST_BYTES as usize)),
        )
}

/// Everything gotcha cannot derive about these routes.
pub fn describe(reg: &mut ExtraResponses) {
    const OFF: &str = "`plugin_disabled`: the plugin is bundled but not on here (or incompatible); `plugin_not_found`: not bundled";
    reg.describe("GET", LIST, 200, "Every plugin this build bundles, as this instance sees it")
        .describe("PUT", ENABLED, 200, "The plugin's status after the switch")
        .error("PUT", ENABLED, 404, "`plugin_not_found`: no such plugin is bundled")
        .error(
            "PUT",
            ENABLED,
            409,
            "`forced_by_config`: the configuration pins it; the switch is read-only",
        )
        .describe("GET", DOCS, 200, "The plugin's documents under `prefix`, without values, by name")
        .error("GET", DOCS, 404, OFF)
        .describe("GET", DOC, 200, "The document")
        .error("GET", DOC, 400, "`invalid`: not a document name")
        .error("GET", DOC, 404, "`doc_not_found`, `plugin_disabled` or `plugin_not_found`")
        .describe("PUT", DOC, 200, "The document as written")
        .error("PUT", DOC, 400, "`invalid`: not a document name, or not a JSON body")
        .error("PUT", DOC, 404, OFF)
        .body::<PluginDocConflict>(
            "PUT",
            DOC,
            409,
            "`conflict`: the document is not at `expectedRevision`; nothing was written",
        )
        .body::<PluginTooLarge>(
            "PUT",
            DOC,
            413,
            "`too_large`: the document, the request or the plugin's store is over its limit",
        )
        .describe("POST", BATCH, 200, "Every document as written, in the order of the writes")
        .error("POST", BATCH, 400, "`invalid`: a bad name, a name twice, or not a JSON body")
        .error("POST", BATCH, 404, OFF)
        .body::<PluginDocConflict>(
            "POST",
            BATCH,
            409,
            "`conflict`: the first write whose revision did not hold; nothing was written",
        )
        .body::<PluginTooLarge>(
            "POST",
            BATCH,
            413,
            "`too_large`: a document, the request or the plugin's store is over its limit",
        )
        .describe("GET", HISTORY, 200, "Replaced versions, newest first, at most fifty")
        .error("GET", HISTORY, 400, "`invalid`: not a document name")
        .error("GET", HISTORY, 404, OFF);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gotcha::axum::body::Body;
    use gotcha::axum::routing::get;
    use tower_service::Service;

    async fn call(router: &mut gotcha::axum::Router, uri: &str) -> (StatusCode, String) {
        let request = gotcha::axum::http::Request::builder().uri(uri).body(Body::empty()).unwrap();
        std::future::poll_fn(|cx| <gotcha::axum::Router as Service<gotcha::axum::http::Request<Body>>>::poll_ready(router, cx))
            .await
            .unwrap();
        let response = router.call(request).await.unwrap();
        let status = response.status();
        let body = gotcha::axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    /// The router matches on the encoded path and decodes the parameter, so
    /// `%2F` is one segment going in and a `/` coming out — which is how a
    /// client names `events/2026-10`.
    #[tokio::test]
    async fn a_percent_encoded_name_is_one_segment_and_arrives_decoded() {
        let echo = |ApiPath((id, name)): ApiPath<(String, String)>| async move { format!("{id}|{name}") };
        let history = |ApiPath((id, name)): ApiPath<(String, String)>| async move { format!("history {id}|{name}") };
        let mut router = gotcha::axum::Router::new().route(DOC, get(echo)).route(HISTORY, get(history));
        assert_eq!(
            call(&mut router, "/api/plugins/tasks/docs/events%2F2026-10").await,
            (StatusCode::OK, "tasks|events/2026-10".to_string())
        );
        assert_eq!(
            call(&mut router, "/api/plugins/tasks/docs/events%2f2026-10%2Fa.b_c-d/history").await,
            (StatusCode::OK, "history tasks|events/2026-10/a.b_c-d".to_string())
        );
        assert_eq!(
            call(&mut router, "/api/plugins/tasks/docs/state").await,
            (StatusCode::OK, "tasks|state".to_string())
        );
        let (status, _) = call(&mut router, "/api/plugins/tasks/docs/events/2026-10").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "an unencoded slash is two segments, and no route");
    }

    #[test]
    fn every_refusal_carries_the_protocol_code() {
        let code = |error: ApiError| (error.status(), error.body()["error"].as_str().unwrap_or_default().to_string());
        assert_eq!(
            code(PluginError::NotFound("x".into()).into()),
            (StatusCode::NOT_FOUND, "plugin_not_found".into())
        );
        assert_eq!(
            code(
                PluginError::Disabled {
                    id: "x".into(),
                    reason: None
                }
                .into()
            ),
            (StatusCode::NOT_FOUND, "plugin_disabled".into())
        );
        assert_eq!(
            code(PluginError::Forced("x".into()).into()),
            (StatusCode::CONFLICT, "forced_by_config".into())
        );
        assert_eq!(
            code(StoreError::DocNotFound("state".into()).into()),
            (StatusCode::NOT_FOUND, "doc_not_found".into())
        );
        assert_eq!(
            code(StoreError::Invalid("bad".into()).into()),
            (StatusCode::BAD_REQUEST, "invalid".into())
        );
        let conflict: ApiError = StoreError::Conflict {
            doc: "state".into(),
            current: None,
        }
        .into();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert_eq!(conflict.body()["doc"], json!("state"));
        assert_eq!(conflict.body()["current"], json!(null), "null, present, when the document does not exist");
        let large: ApiError = StoreError::TooLarge {
            message: "big".into(),
            limit: 7,
        }
        .into();
        assert_eq!(large.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(large.body(), &json!({"error": "too_large", "message": "big", "limit": 7}));
    }

    #[tokio::test]
    async fn an_oversized_body_is_the_stores_413_and_bad_json_is_invalid() {
        let accept = |PluginJson(body): PluginJson<PluginPutDoc>| async move { body.value.to_string() };
        let mut router = gotcha::axum::Router::new()
            .route("/put", gotcha::axum::routing::put(accept))
            .layer(gotcha::axum::extract::DefaultBodyLimit::max(64));
        let mut send = |body: String| {
            let request = gotcha::axum::http::Request::builder()
                .method("PUT")
                .uri("/put")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap();
            let fut = router.call(request);
            async move {
                let response = fut.await.unwrap();
                let status = response.status();
                let body = gotcha::axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
                (status, serde_json::from_slice::<serde_json::Value>(&body).unwrap_or_default())
            }
        };
        let (status, body) = send(r#"{"expectedRevision":null,"value":1}"#.into()).await;
        assert_eq!((status, body), (StatusCode::OK, json!(1)), "the handler echoed the value back");
        let (status, body) = send(format!(r#"{{"expectedRevision":null,"value":"{}"}}"#, "x".repeat(100))).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(body["error"], json!("too_large"));
        assert_eq!(body["limit"], json!(MAX_REQUEST_BYTES));
        let (status, body) = send("{".into()).await;
        assert_eq!((status, body["error"].clone()), (StatusCode::BAD_REQUEST, json!("invalid")));
    }
}
