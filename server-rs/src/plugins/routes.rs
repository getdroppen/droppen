//! `/api/plugins/*`: installation, the plugin switches and the document
//! store, over HTTP.
//!
//! Registered once and generic — nothing here names a plugin — so the OpenAPI
//! document is the same whichever plugins an instance has installed. Every
//! `/api/plugins/{id}/…` call goes through [`super::Plugins::require_enabled`]
//! first, so a plugin that is not on answers `404 plugin_disabled` and never
//! gets a store made for it. The contract is
//! `docs/plugins/storage-protocol.md`.
//!
//! A document name is one path segment: a client sends `events/2026-10` as
//! `events%2F2026-10`, and the router hands the handler the decoded name.
//!
//! # Not to the cloud
//!
//! A request that came over the cloud agent's bridge carries a `cloud:<device>`
//! identity. Every handler here refuses one with `404 plugin_not_found`, so a
//! route the bridge's own allowlist ever lets through by mistake still answers
//! nothing (`docs/plugins/runtime.md` §5). The asset route
//! ([`super::serve`]) does the same.

use std::sync::Arc;

use gotcha::axum::body::Bytes;
use gotcha::axum::extract::{FromRequest, Request};
use gotcha::axum::http::StatusCode;
use gotcha::axum::middleware::Next;
use gotcha::axum::response::{IntoResponse, Response};
use gotcha::oas::{Parameter, RequestBody};
use gotcha::{api, Either, Json, ParameterProvider, State};
use serde::de::DeserializeOwned;
use serde_json::json;

use super::install::{InstallError, MAX_TARBALL_BYTES};
use super::store::{StoreError, MAX_REQUEST_BYTES};
use super::wire::{
    PluginBatch, PluginBatchResult, PluginCurrentDoc, PluginDeleted, PluginDoc, PluginDocConflict, PluginDocHistory, PluginDocList, PluginDocQuery,
    PluginHistoryQuery, PluginInvalidPackage, PluginList, PluginPutDoc, PluginStatus, PluginSwitch, PluginTarballBody, PluginTooLarge,
};
use super::{DecisionReason, PluginError, Plugins};
use crate::api::error::ApiError;
use crate::api::extract::{ApiJson, ApiPath, ApiQuery};
use crate::api::{ApiRouter, AppState};
use crate::auth::MaybeIdentity;
use crate::openapi_ext::ExtraResponses;
use droppen_plugin_sdk::codes;

const LIST: &str = "/api/plugins";
const PLUGIN: &str = "/api/plugins/{id}";
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
            PluginError::Install(InstallError::NotFound(_)) => coded(StatusCode::NOT_FOUND, codes::PLUGIN_NOT_FOUND, message),
            PluginError::Install(InstallError::TooLarge { message, limit }) => too_large(message, limit),
            PluginError::Install(InstallError::Refused(_)) => coded(StatusCode::BAD_REQUEST, codes::INVALID_PACKAGE, message),
            PluginError::Install(InstallError::Failed(detail)) => {
                // The detail names this instance's directories; the log is
                // where an operator can see it, the body is not.
                tracing::error!(%detail, "a plugin install failed");
                coded(StatusCode::INTERNAL_SERVER_ERROR, "failed", "the install failed; the server log says why")
            }
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

/// The body of `POST /api/plugins`: the package's raw bytes, held to
/// [`MAX_TARBALL_BYTES`], whose overflow is `413 too_large`.
pub struct PluginTarball(pub Bytes);

impl<S> FromRequest<S> for PluginTarball
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        Bytes::from_request(request, state).await.map(PluginTarball).map_err(|rejection| {
            if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
                too_large(format!("one install may carry {MAX_TARBALL_BYTES} bytes"), MAX_TARBALL_BYTES as u64)
            } else {
                coded(StatusCode::BAD_REQUEST, codes::INVALID_PACKAGE, rejection.body_text())
            }
        })
    }
}

impl Plugins {
    /// Run a blocking store call off the runtime: a write is an fsync.
    async fn blocking<T: Send + 'static>(
        self: &Arc<Self>,
        work: impl FnOnce(&Plugins) -> Result<T, PluginError> + Send + 'static,
    ) -> Result<T, ApiError> {
        let plugins = self.clone();
        tokio::task::spawn_blocking(move || work(&plugins))
            .await
            .map_err(|error| ApiError::internal(error.to_string()))?
            .map_err(ApiError::from)
    }
}

impl MaybeIdentity {
    /// Refuse a request that came over the cloud agent's bridge: a device gets
    /// none of the plugin surface, and the answer is the one an unknown plugin
    /// gets so it learns nothing from the difference.
    fn refuse_cloud(&self) -> Result<(), ApiError> {
        if self.is_cloud() {
            return Err(coded(
                StatusCode::NOT_FOUND,
                codes::PLUGIN_NOT_FOUND,
                "no plugin is available to a bridged request",
            ));
        }
        Ok(())
    }

    /// The signed-in user behind a request, when there is one.
    fn caller(&self) -> Option<String> {
        self.0.as_ref().map(|identity| identity.username.clone())
    }
}

/// [`MaybeIdentity::refuse_cloud`], before the request's body is parsed.
///
/// A handler's arguments — including a JSON body — are extracted before its
/// first line runs, so a malformed body from a device would otherwise be
/// answered `400` before the handler could refuse it. This layer sits inside
/// the authentication guard (which has put the identity in the request
/// extensions by now) and outside every extractor, so a device gets the same
/// `404 plugin_not_found` whatever it sent. The handler-level check stays as
/// the second lock on the same door.
async fn refuse_cloud_requests(request: Request, next: Next) -> Response {
    if request
        .extensions()
        .get::<crate::auth::AuthIdentity>()
        .is_some_and(|identity| identity.username.starts_with("cloud:"))
    {
        return coded(
            StatusCode::NOT_FOUND,
            codes::PLUGIN_NOT_FOUND,
            "no plugin is available to a bridged request",
        )
        .into_response();
    }
    next.run(request).await
}

/// Every plugin installed in this instance, as this instance sees it.
#[api(id = "list_plugins", group = "plugins", summary = "List plugins")]
pub async fn list_plugins(State(state): State<AppState>, #[api(skip)] who: MaybeIdentity) -> Result<Json<PluginList>, ApiError> {
    who.refuse_cloud()?;
    Ok(Json(PluginList {
        plugins: state.plugins.status_async().await.into_iter().map(PluginStatus::from).collect(),
    }))
}

/// Install a plugin package: a gzip'd tar whose entries are under `package/`.
///
/// Replaces an installed plugin of the same id, atomically, and switches it
/// on; a refused package leaves the previous install as it was. The bytes are
/// the tarball itself, not multipart: `application/gzip` or
/// `application/octet-stream`.
#[api(id = "install_plugin", group = "plugins", summary = "Install a plugin")]
pub async fn install_plugin(
    State(state): State<AppState>,
    #[api(skip)] who: MaybeIdentity,
    #[api(skip)] PluginTarball(tarball): PluginTarball,
) -> Result<Json<PluginStatus>, ApiError> {
    who.refuse_cloud()?;
    let by = who.caller();
    state.plugins.blocking( move |plugins| plugins.install(&tarball, by.as_deref()))
        .await
        .map(|status| Json(status.into()))
}

/// Uninstall a plugin. Its documents and its on/off decision stay, for a
/// later install.
#[api(id = "uninstall_plugin", group = "plugins", summary = "Uninstall a plugin")]
pub async fn uninstall_plugin(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    #[api(skip)] who: MaybeIdentity,
) -> Result<PluginDeleted, ApiError> {
    who.refuse_cloud()?;
    state.plugins.blocking( move |plugins| plugins.uninstall(&id)).await?;
    Ok(PluginDeleted)
}

/// Turn a plugin on or off for this instance. Any signed-in user may.
#[api(id = "set_plugin_enabled", group = "plugins", summary = "Switch a plugin")]
pub async fn set_plugin_enabled(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    #[api(skip)] who: MaybeIdentity,
    ApiJson(body): ApiJson<PluginSwitch>,
) -> Result<Json<PluginStatus>, ApiError> {
    who.refuse_cloud()?;
    let by = who.caller();
    state.plugins.blocking( move |plugins| {
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
    #[api(skip)] who: MaybeIdentity,
) -> Result<Json<PluginDocList>, ApiError> {
    who.refuse_cloud()?;
    let prefix = query.prefix.unwrap_or_default();
    state.plugins.blocking( move |plugins| plugins.list(&id, &prefix)).await.map(|docs| {
        Json(PluginDocList {
            docs: docs.into_iter().map(Into::into).collect(),
        })
    })
}

/// One document.
#[api(id = "get_plugin_doc", group = "plugins", summary = "Read a plugin document")]
pub async fn get_plugin_doc(
    State(state): State<AppState>,
    ApiPath((id, name)): ApiPath<(String, String)>,
    #[api(skip)] who: MaybeIdentity,
) -> Result<Json<PluginDoc>, ApiError> {
    who.refuse_cloud()?;
    state.plugins.blocking( move |plugins| plugins.get(&id, &name))
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
    who.refuse_cloud()?;
    let by = who.caller();
    state.plugins.blocking( move |plugins| plugins.put(&id, &name, body.into(), by.as_deref()))
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
    who.refuse_cloud()?;
    let by = who.caller();
    let writes = body.writes.into_iter().map(Into::into).collect();
    state.plugins.blocking( move |plugins| plugins.batch(&id, writes, by.as_deref()))
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
    #[api(skip)] who: MaybeIdentity,
) -> Result<Json<PluginDocHistory>, ApiError> {
    who.refuse_cloud()?;
    let offset = query.offset.unwrap_or(0) as usize;
    let limit = query.limit.map(|limit| limit as usize);
    state.plugins.blocking( move |plugins| plugins.history(&id, &name, offset, limit))
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
/// body for the limit to matter to. The install route gets a sub-router of its
/// own for its own, larger, limit.
///
/// The asset route (`/plugins/…`) is registered here too, so the router claims
/// it before the SPA fallback can hand a page its shell instead of a module.
///
/// The whole plugin surface is one sub-router with [`refuse_cloud_requests`]
/// over it, so a bridged request is turned away before any body is parsed;
/// the routers are then merged into the API's.
pub fn routes(router: ApiRouter) -> ApiRouter {
    let plugins = ApiRouter::default()
        .get(LIST, list_plugins)
        .delete(PLUGIN, uninstall_plugin)
        .put(ENABLED, set_plugin_enabled)
        .get(DOCS, list_plugin_docs)
        .get(HISTORY, plugin_doc_history)
        .merge(
            ApiRouter::default()
                .post(LIST, install_plugin)
                .layer(gotcha::axum::extract::DefaultBodyLimit::max(MAX_TARBALL_BYTES)),
        )
        .merge(
            ApiRouter::default()
                .get(DOC, get_plugin_doc)
                .put(DOC, put_plugin_doc)
                .post(BATCH, plugin_batch)
                .layer(gotcha::axum::extract::DefaultBodyLimit::max(MAX_REQUEST_BYTES as usize)),
        )
        .get(super::serve::ASSET_PATH, super::serve::serve_asset)
        .head(super::serve::ASSET_PATH, super::serve::serve_asset)
        .layer(gotcha::axum::middleware::from_fn(refuse_cloud_requests));
    router.merge(plugins)
}

/// Everything gotcha cannot derive about these routes.
pub fn describe(reg: &mut ExtraResponses) {
    const OFF: &str = "`plugin_disabled`: installed but not on here (or incompatible); `plugin_not_found`: not installed";
    reg.describe_final("GET", LIST, 200, "Every plugin installed in this instance, as this instance sees it")
        .describe("POST", LIST, 200, "The plugin as installed and switched on")
        .body::<PluginInvalidPackage>("POST", LIST, 400, "`invalid_package`: the package was refused; `message` says why")
        .body::<PluginTooLarge>("POST", LIST, 413, "`too_large`: the tarball is over 32 MiB")
        .request_body::<PluginTarballBody>(
            "POST",
            LIST,
            "application/octet-stream",
            "The plugin package: a gzip'd tar whose entries are under `package/` (`application/gzip` is the same bytes)",
        )
        .restatus(
            "DELETE",
            PLUGIN,
            200,
            204,
            "The plugin is uninstalled; its documents and its on/off decision are kept",
        )
        .error("DELETE", PLUGIN, 404, "`plugin_not_found`: no plugin of that id is installed")
        .describe("PUT", ENABLED, 200, "The plugin's status after the switch")
        .error("PUT", ENABLED, 404, "`plugin_not_found`: no plugin of that id is installed")
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
    use gotcha::axum::http::Request;
    use tower_service::Service;

    async fn call(router: &mut gotcha::axum::Router, uri: &str) -> (StatusCode, String) {
        let request = Request::builder().uri(uri).body(Body::empty()).unwrap();
        std::future::poll_fn(|cx| <gotcha::axum::Router as Service<Request<Body>>>::poll_ready(router, cx))
            .await
            .unwrap();
        let response = router.call(request).await.unwrap();
        let status = response.status();
        let body = gotcha::axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    /// The selectors the router matches on the request as sent, and decodes
    /// for the handler: `%2F` is one segment going in and a `/` coming out,
    /// which is how a client names `events/2026-10`.
    #[tokio::test]
    async fn a_percent_encoded_name_is_one_segment_and_arrives_decoded() {
        use gotcha::axum::routing::get;
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
        assert_eq!(
            code(PluginError::Install(InstallError::Refused("no droppen".into())).into()),
            (StatusCode::BAD_REQUEST, "invalid_package".into())
        );
        assert_eq!(
            code(PluginError::Install(InstallError::NotFound("tasks".into())).into()),
            (StatusCode::NOT_FOUND, "plugin_not_found".into())
        );
        let large: ApiError = PluginError::Install(InstallError::TooLarge {
            message: "too big".into(),
            limit: 7,
        })
        .into();
        assert_eq!(large.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(large.body()["limit"], json!(7));
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

    /// A failed install's `500` names no instance directory: the detail goes
    /// to the log, where an operator can see it.
    #[test]
    fn a_failed_install_does_not_put_instance_paths_in_its_body() {
        let error: ApiError = PluginError::Install(InstallError::Failed(
            "/home/someone/.local/share/remote-hand/instances/t/plugin-packages/.tmp-1: disk full".into(),
        ))
        .into();
        assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error.body()["error"], json!("failed"));
        let message = error.body()["message"].as_str().unwrap_or_default();
        assert!(!message.contains('/'), "{message:?}");
        assert!(!message.contains(".local"), "{message:?}");
    }

    #[tokio::test]
    async fn an_oversized_body_is_the_stores_413_and_bad_json_is_invalid() {
        let accept = |PluginJson(body): PluginJson<PluginPutDoc>| async move { body.value.to_string() };
        let mut router = gotcha::axum::Router::new()
            .route("/put", gotcha::axum::routing::put(accept))
            .layer(gotcha::axum::extract::DefaultBodyLimit::max(64));
        let mut send = |body: String| {
            let request = Request::builder()
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

    /// The whole HTTP surface of an install: the tarball goes up, the status
    /// comes back with its entry, the list agrees, and the delete removes it.
    #[tokio::test]
    async fn an_install_is_listed_with_its_entry_and_a_delete_uninstalls_it() {
        use crate::plugins::tests::package_bytes;
        use crate::plugins::testing::{package, tarball};

        let home = tempfile::tempdir().unwrap();
        let (state, mut router) = crate::api::test_support::guarded_router(home.path(), Some(false)).await;
        let send = |router: &mut gotcha::axum::Router, method: &'static str, uri: &str, body: Vec<u8>| {
            let request = Request::builder().method(method).uri(uri).body(Body::from(body)).unwrap();
            let fut = router.call(request);
            async move {
                let response = fut.await.unwrap();
                let status = response.status();
                let body = gotcha::axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
                (status, serde_json::from_slice::<serde_json::Value>(&body).unwrap_or_default())
            }
        };

        let (status, body) = send(&mut router, "POST", LIST, package_bytes("tasks", "0.1.0")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["id"], json!("tasks"));
        assert_eq!(body["build"].as_str().unwrap().len(), 18);
        assert_eq!(body["aliases"], json!(["/tasks"]));
        assert_eq!(body["permissions"], json!(["storage"]));
        let entry = body["entry"].as_str().expect("an entry").to_string();

        let (status, list) = send(&mut router, "GET", LIST, Vec::new()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(list["plugins"][0]["entry"], json!(entry));

        // The entry answers over the same router, to a caller with no session:
        // the key in the path is the credential.
        let (status, _) = call(&mut router, &entry).await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = send(&mut router, "DELETE", "/api/plugins/tasks", Vec::new()).await;
        assert_eq!((status, body), (StatusCode::NO_CONTENT, serde_json::Value::Null));
        let (status, list) = send(&mut router, "GET", LIST, Vec::new()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(list["plugins"], json!([]));
        let (status, body) = send(&mut router, "DELETE", "/api/plugins/tasks", Vec::new()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], json!("plugin_not_found"));

        // A refused package is 400 invalid_package and changes nothing.
        let mut broken = package("notes", "0.1.0");
        broken["droppen"]["id"] = json!("Notes");
        let (status, body) = send(&mut router, "POST", LIST, tarball(&broken, &[("dist/index.js", b"x")])).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], json!("invalid_package"));
        assert!(body["message"].as_str().unwrap().contains("is not a plugin id"));
        let _ = state;
    }

    #[tokio::test]
    async fn a_tarball_over_the_limit_is_413_too_large() {
        use crate::plugins::install::MAX_TARBALL_BYTES;
        let home = tempfile::tempdir().unwrap();
        let (_state, mut router) = crate::api::test_support::guarded_router(home.path(), Some(false)).await;
        let request = Request::builder()
            .method("POST")
            .uri(LIST)
            .body(Body::from(vec![0u8; MAX_TARBALL_BYTES + 1]))
            .unwrap();
        let response = router.call(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body = gotcha::axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"], json!("too_large"));
        assert_eq!(body["limit"], json!(MAX_TARBALL_BYTES));
    }

    #[tokio::test]
    async fn a_cloud_identity_is_refused_everywhere() {
        use gotcha::axum::extract::Request as ExtractRequest;
        use gotcha::axum::middleware::Next;
        use gotcha::axum::response::Response as HttpResponse;

        async fn as_cloud(mut request: ExtractRequest, next: Next) -> HttpResponse<Body> {
            request.extensions_mut().insert(crate::auth::AuthIdentity {
                username: "cloud:device-1".into(),
                token_sha256: None,
            });
            next.run(request).await
        }

        // The handler-level check is the second lock on the same door.
        let cloud = MaybeIdentity(Some(crate::auth::AuthIdentity {
            username: "cloud:device-1".into(),
            token_sha256: None,
        }));
        let refused = cloud.refuse_cloud().expect_err("a bridged identity is refused");
        assert_eq!((refused.status(), refused.body()["error"].clone()), (StatusCode::NOT_FOUND, json!("plugin_not_found")));
        assert!(MaybeIdentity(None).refuse_cloud().is_ok());

        let home = tempfile::tempdir().unwrap();
        let (state, router) = crate::api::test_support::guarded_router(home.path(), Some(false)).await;
        state.plugins.install(&crate::plugins::tests::package_bytes("tasks", "0.1.0"), None).unwrap();
        let mut router = router.layer(gotcha::axum::middleware::from_fn(as_cloud));
        // Empty or malformed bodies on purpose: the refusal must come before
        // the extractors, so these are 404 and never 400.
        for (method, uri, body) in [
            ("GET", LIST, Body::empty()),
            ("POST", LIST, Body::empty()),
            ("PUT", "/api/plugins/tasks/enabled", Body::empty()),
            ("PUT", "/api/plugins/tasks/docs/state", Body::empty()),
            ("POST", "/api/plugins/tasks/batch", Body::empty()),
            ("GET", "/api/plugins/tasks/docs", Body::empty()),
            ("GET", "/api/plugins/tasks/docs/state", Body::empty()),
            ("DELETE", "/api/plugins/tasks", Body::empty()),
        ] {
            let request = Request::builder().method(method).uri(uri).body(body).unwrap();
            let response = router.call(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {uri}");
            let body = gotcha::axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            assert_eq!(body["error"], json!("plugin_not_found"), "{method} {uri}");
        }
    }
}
