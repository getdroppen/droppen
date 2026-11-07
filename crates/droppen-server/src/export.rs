//! `export-openapi`: write the served spec to a file.
//!
//! The document is built by the same route registration the server assembles
//! ([`crate::app::routes`]), and gotcha's [`into_openapi`] runs the same
//! transforms the documentation endpoint does. No listener and no request are
//! involved, so the export cannot be captured by a proxy, cannot be refused by
//! the authentication guard, and needs no instance. `tests/probe.rs` pins the
//! served document against `openapi/openapi.json`, and a test below pins the
//! served document against this export.
//!
//! [`into_openapi`]: gotcha::GotchaRouter::into_openapi

use std::path::PathBuf;

use clap::Args;
use gotcha::GotchaRouter;

use crate::{GotchaError, GotchaResult};

/// The `export-openapi` command line.
#[derive(Debug, PartialEq, Eq, Args)]
#[command(args_override_self = true)]
pub struct ExportOpenapiArgs {
    /// The file to write the spec to (stdout without it).
    #[arg(long, allow_hyphen_values = true)]
    pub out: Option<PathBuf>,
}

impl ExportOpenapiArgs {
    /// Write the spec the server serves to `--out`, or to standard output.
    pub async fn run(self) -> GotchaResult<()> {
        let spec = openapi_json()?;
        match self.out {
            Some(path) => {
                if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    tokio::fs::create_dir_all(parent).await.map_err(GotchaError::Io)?;
                }
                tokio::fs::write(&path, &spec).await.map_err(GotchaError::Io)?;
                // Only here: the logger writes to standard output, which is
                // the spec itself when there is no `--out`.
                gotcha::default_logger()?;
                tracing::info!(path = %path.display(), bytes = spec.len(), "wrote openapi spec");
            }
            None => print!("{spec}"),
        }
        Ok(())
    }
}

/// The document the server serves, serialized as `openapi/openapi.json` keeps
/// it: pretty-printed, every object sorted, with a trailing newline.
fn openapi_json() -> GotchaResult<String> {
    let spec = crate::app::routes(GotchaRouter::default()).into_openapi();
    let mut value = serde_json::to_value(&spec).map_err(|error| GotchaError::message(error.to_string()))?;
    // gotcha builds some maps (a schema's `properties`) from hash maps, and a
    // `Value` keeps the order it was given (`preserve_order`), so sorting is
    // what makes the exported file the same on every run and diffable. The
    // routes' own transform sorts too; sorting again fixes the order of the
    // bytes this function writes.
    value.sort_all_objects();
    let mut pretty = serde_json::to_string_pretty(&value).map_err(|error| GotchaError::message(error.to_string()))?;
    pretty.push('\n');
    Ok(pretty)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The export and the server build their documents from the same routes
    /// and transforms: what this writes is what a request to `/openapi.json`
    /// answers.
    #[tokio::test]
    async fn the_export_is_the_document_the_server_serves() {
        use tower_service::Service;

        let home = tempfile::tempdir().expect("tempdir");
        let (_, mut router) = crate::api::test_support::guarded_router(home.path(), Some(false)).await;
        let request = gotcha::axum::http::Request::get("/openapi.json")
            .body(gotcha::axum::body::Body::empty())
            .expect("request");
        let response = router.call(request).await.expect("infallible");
        assert!(response.status().is_success(), "/openapi.json: {}", response.status());
        let bytes = gotcha::axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let served: serde_json::Value = serde_json::from_slice(&bytes).expect("the served document is JSON");
        let exported: serde_json::Value = serde_json::from_str(&openapi_json().expect("the export")).expect("the exported document is JSON");
        assert_eq!(exported, served, "the export drifted from the document the server serves");
    }
}
