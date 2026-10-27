//! `export-openapi`: write the served spec to a file.
//!
//! gotcha keeps both `GotchaRouter::operations` and `into_axum_router` — the
//! only places the assembled spec exists — crate-private
//! (`gotcha-0.4.1/src/router.rs:38` and `:263`), and mounts the document as a
//! route closure rather than exposing it. There is therefore no way to obtain
//! the spec without going through HTTP.
//!
//! So the export *is* a request: bind port 0, serve the real router in-process,
//! `GET /openapi.json`, write, stop. That is not just a workaround — it also
//! guarantees the exported document is byte-for-byte the one the server serves,
//! which a separate generation path could drift from.

use std::path::PathBuf;

use remote_hand_server::app::RemoteHandApp;
use remote_hand_server::{GotchaError, GotchaResult};

/// Fetch `/openapi.json` from a throwaway in-process server and write it out.
pub async fn export_openapi(out: Option<PathBuf>) -> GotchaResult<()> {
    let app = RemoteHandApp {
        port_override: Some(0),
        // The export is a read of this process's own router, not a server: it
        // must not refuse to run because a real server owns the instance, and it
        // must not claim ownership itself.
        skip_lock: true,
        // Same shape, same reason: on a machine that has accounts, the guard
        // would answer this process's own `GET /openapi.json` with a `401` and
        // `pnpm gen:api` would fail on exactly the machines where the feature is
        // switched on.
        auth_disabled: true,
        ..Default::default()
    };
    let prepared = app.prepare().await?;
    let addr = prepared.addr;
    let cancel = prepared.cancel();
    let server = tokio::spawn(prepared.serve_task());

    let result = fetch_spec(addr).await;
    cancel.cancel();
    let _ = server.await;

    let spec = result.map_err(GotchaError::message)?;
    match out {
        Some(path) => {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                tokio::fs::create_dir_all(parent).await.map_err(GotchaError::Io)?;
            }
            tokio::fs::write(&path, &spec).await.map_err(GotchaError::Io)?;
            tracing::info!(path = %path.display(), bytes = spec.len(), "wrote openapi spec");
        }
        None => print!("{spec}"),
    }
    Ok(())
}

async fn fetch_spec(addr: std::net::SocketAddr) -> Result<String, String> {
    let url = format!("http://{addr}/openapi.json");
    let body = reqwest::get(&url)
        .await
        .map_err(|e| format!("GET {url}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("GET {url}: {e}"))?
        .text()
        .await
        .map_err(|e| format!("GET {url}: {e}"))?;
    let value: serde_json::Value = serde_json::from_str(&body).map_err(|e| format!("/openapi.json is not JSON: {e}"))?;
    // Re-serialize rather than passing the response through: `serde_json`'s map
    // is a `BTreeMap`, so this both pretty-prints and gives a stable key order,
    // which is what makes the exported file diffable across runs.
    let mut pretty = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    pretty.push('\n');
    Ok(pretty)
}
