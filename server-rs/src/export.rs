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

use clap::Args;

use crate::app::RemoteHandApp;
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
    /// Fetch `/openapi.json` from a throwaway in-process server and write it out.
    pub async fn run(self) -> GotchaResult<()> {
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
        match self.out {
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
    // Exactly `200`: a redirect is an answer to report, not one to follow.
    if status != reqwest::StatusCode::OK {
        return Err(format!("GET /openapi.json -> {:?} {status}", response.version()));
    }
    let raw = response.bytes().await.map_err(|e| format!("read: {e}"))?;
    let body = String::from_utf8(raw.to_vec()).map_err(|e| format!("non-utf8 response: {e}"))?;
    let mut value: serde_json::Value = serde_json::from_str(&body).map_err(|e| format!("/openapi.json is not JSON: {e}"))?;
    // Re-serialize rather than passing the response through, with every object
    // sorted: gotcha builds some maps (a schema's `properties`) from hash maps,
    // and a `Value` keeps the order it was given (`preserve_order`), so sorting
    // is what makes the exported file the same on every run and diffable.
    value.sort_all_objects();
    let mut pretty = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    pretty.push('\n');
    Ok(pretty)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where [`a_proxy_in_the_environment_does_not_capture_the_export`] runs the
    /// export, when this test binary runs it as a process of its own.
    const EXPORT_DIR: &str = "DROPPEN_UNIT_TEST_EXPORT_DIR";

    /// The export is a request to a port this process just opened, so the
    /// environment's proxy is not part of it. With `HTTP_PROXY`/`ALL_PROXY`
    /// pointed at a closed port, only a client built with `no_proxy()` reaches
    /// the server and writes the spec.
    ///
    /// The variables are a process's, so the export runs in a process of its
    /// own: this test binary again, for this test alone, with them set. Set in
    /// the test's own process they would be every other test's too under
    /// `cargo test`, which runs the tests as threads of one process: a server
    /// another test built meanwhile kept its accounts and sessions in this
    /// test's directories, beside every other such server's.
    #[tokio::test]
    async fn a_proxy_in_the_environment_does_not_capture_the_export() {
        let Ok(dir) = std::env::var(EXPORT_DIR) else {
            let dir = tempfile::tempdir().expect("tempdir");
            // libtest names a test by its path inside the crate.
            let (_crate, module) = module_path!().split_once("::").expect("a module inside a crate");
            let status = std::process::Command::new(std::env::current_exe().expect("this test binary"))
                .args(["--exact", "--quiet", &format!("{module}::a_proxy_in_the_environment_does_not_capture_the_export")])
                .env(EXPORT_DIR, dir.path())
                // Keep the throwaway server's paths inside the test's own directory.
                .env("DROPPEN_CONFIG_DIR", dir.path().join("config"))
                .env("DROPPEN_DATA_DIR", dir.path().join("data"))
                .env("APP_HOME_DIR", dir.path().join("home"))
                .env("APP_INSTANCE", "openapi-export")
                .env("HTTP_PROXY", "http://127.0.0.1:1")
                .env("ALL_PROXY", "http://127.0.0.1:1")
                .env("NO_PROXY", "")
                .status()
                .expect("this test binary runs");
            assert!(status.success(), "the export, in a process of its own: {status}");
            // Read here, so a run that never reached the export fails too.
            let spec = std::fs::read_to_string(dir.path().join("openapi.json")).expect("the written spec");
            assert!(spec.contains("\"openapi\""), "the file is the spec, not a proxy error page");
            return;
        };
        ExportOpenapiArgs {
            out: Some(std::path::Path::new(&dir).join("openapi.json")),
        }
        .run()
        .await
        .expect("the export must ignore the environment's proxy");
    }
}
