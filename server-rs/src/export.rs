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
    // Loopback, in this process. A client that honoured `HTTP_PROXY`/`ALL_PROXY`
    // would send the request to whatever the environment names instead, and a
    // redirect would take the spec somewhere else; neither is something the
    // export means, so both are off. The same client shape the other loopback
    // callers use (`static_files`, `aggregator::hub`, `aggregator::copies`).
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("connect: {e}"))?;
    let response = client.get(&url).send().await.map_err(|e| format!("connect: {e}"))?;
    let status = response.status();
    // Exactly `200`, as the raw HTTP/1.1 reader this replaced accepted: a
    // redirect is an answer to report, not one to follow.
    if status != reqwest::StatusCode::OK {
        return Err(format!("GET /openapi.json -> {:?} {status}", response.version()));
    }
    let raw = response.bytes().await.map_err(|e| format!("read: {e}"))?;
    let body = String::from_utf8(raw.to_vec()).map_err(|e| format!("non-utf8 response: {e}"))?;
    let value: serde_json::Value = serde_json::from_str(&body).map_err(|e| format!("/openapi.json is not JSON: {e}"))?;
    // Re-serialize rather than passing the response through: `serde_json`'s map
    // is a `BTreeMap`, so this both pretty-prints and gives a stable key order,
    // which is what makes the exported file diffable across runs.
    let mut pretty = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    pretty.push('\n');
    Ok(pretty)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The export is a request to a port this process just opened, so the
    /// environment's proxy is not part of it. With `HTTP_PROXY`/`ALL_PROXY`
    /// pointed at a closed port, only a client built with `no_proxy()` reaches
    /// the server and writes the spec.
    ///
    /// The variables are set for the whole process, which is safe under the
    /// suite's process-per-test runner (see `server-rs/.config/nextest.toml`).
    #[tokio::test]
    async fn a_proxy_in_the_environment_does_not_capture_the_export() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("openapi.json");
        let set = |name: &str, value: &str| std::env::set_var(name, value);
        // Keep the throwaway server's paths inside the test's own directory.
        set("DROPPEN_CONFIG_DIR", dir.path().join("config").to_str().expect("utf-8"));
        set("DROPPEN_DATA_DIR", dir.path().join("data").to_str().expect("utf-8"));
        set("APP_HOME_DIR", dir.path().join("home").to_str().expect("utf-8"));
        set("APP_INSTANCE", "openapi-export");
        set("HTTP_PROXY", "http://127.0.0.1:1");
        set("ALL_PROXY", "http://127.0.0.1:1");
        set("NO_PROXY", "");
        let exported = export_openapi(Some(out.clone())).await;
        for name in ["DROPPEN_CONFIG_DIR", "DROPPEN_DATA_DIR", "APP_HOME_DIR", "APP_INSTANCE", "HTTP_PROXY", "ALL_PROXY", "NO_PROXY"] {
            std::env::remove_var(name);
        }
        exported.expect("the export must ignore the environment's proxy");
        let spec = std::fs::read_to_string(&out).expect("the written spec");
        assert!(spec.contains("\"openapi\""), "the file is the spec, not a proxy error page");
    }
}
