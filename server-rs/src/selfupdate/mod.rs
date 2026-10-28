//! Replacing the running binary from inside the running server.
//!
//! This closes the loop the rest of restart-survival opened. Terminals, agent
//! runs and embedded apps all survive the server process ending; once that is
//! true, upgrading the server stops being "stop the container, pull, start
//! again, lose everything" and becomes "swap the binary, exit, let the
//! supervisor start the new one" — with the user's shells still attached to the
//! same shims on the other side.
//!
//! # The shape of an upgrade
//!
//! 1. The new binary is uploaded and written to a staging path in
//!    [`store::BinStore`], never to the installed name.
//! 2. If the caller sent a `sha256`, it must match what actually arrived.
//! 3. The staged binary is made to prove it *works on this machine*: `--version`
//!    must exit 0 and print something, and `preflight` must exit 0, which means
//!    it also loaded this server's configuration. A binary for the wrong
//!    architecture, a truncated upload and a build that cannot read the config
//!    file all fail here, while the incumbent is still the installed one.
//! 4. The incumbent is copied to `.prev` and the staged binary is `rename`d over
//!    the installed name — atomic, so there is no moment where the installed
//!    path is a partial file.
//! 5. The response goes out, and then — only if this server is supervised — the
//!    process stops itself gracefully. The supervisor sees a clean exit and
//!    starts the binary that is now installed.
//!
//! Step 5 is deliberately *after* the response. A server that exits before its
//! answer has flushed leaves the client unable to tell a successful upgrade from
//! a crash, which is the one failure mode that makes people stop trusting the
//! feature.
//!
//! # Downgrades
//!
//! A binary without `preflight` fails the health check and is refused. That is
//! the honest behaviour rather than a bug: this route cannot verify a binary
//! that predates the contract it verifies against. Rolling further back means
//! restoring an older binary by hand, or deleting
//! `<data>/bin/remote-hand-server` so the one baked into the running program
//! seeds again.
//!
//! # Privilege
//!
//! This route replaces the program the machine will run, which makes it the most
//! privileged route this server has. It is reached like every other one: through
//! [`crate::auth`]'s layer, so only a signed-in account on this instance may post
//! to it. Anyone who *is* signed in can already read every file under the home
//! directory, open a shell through `POST /api/terminals`, and run arbitrary
//! commands as this user; a route that swaps the binary adds persistence to
//! that, not a new capability. Persistence is still a meaningful escalation, so
//! the port itself belongs behind an SSH tunnel or a private network. Same-origin
//! checks (`crate::origin`) apply here as to every other route, so a web page the
//! user visits cannot post to it.

pub mod store;

use std::path::Path;
use std::time::Duration;

use gotcha::{api, Json, Multipart, Schematic, State};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::text::hex;
use tokio::io::AsyncWriteExt;

use crate::api::error::ApiError;
use crate::api::{ApiRouter, AppState};
use crate::openapi_ext::ExtraResponses;
use crate::sessions::Paths;

use store::BinStore;

/// Ceiling on an uploaded binary.
///
/// A release build is tens of megabytes; the headroom is for a debug build,
/// which is the one a developer dogfooding this will actually reach for.
const BINARY_LIMIT: u64 = 512 * 1024 * 1024;

/// Headroom over [`BINARY_LIMIT`] for the multipart framing around it, so a
/// binary *at* the ceiling still arrives and this handler's own message — not a
/// bare transport `413` — is what answers anything over it.
const MULTIPART_OVERHEAD: u64 = 64 * 1024;

/// How long each health-check run of the staged binary may take.
///
/// Generous for what it does — printing a version and reading one config file —
/// because the alternative to waiting is refusing a good binary on a loaded box.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the response is given to reach the client before the process stops.
///
/// Long enough for axum to have written and flushed the body on a loopback or
/// LAN connection, short enough that the user is not left wondering. It is a
/// delay rather than a handshake because there is nothing to hand shake with:
/// the client that asked for this is about to lose the connection either way.
const FLUSH_GRACE: Duration = Duration::from_millis(600);

/// How much of each of the staged binary's pipes is kept. See [`run`].
const CAPTURE_CAP: usize = 64 * 1024;

/// The longest a `--version` line may plausibly be.
const MAX_VERSION: usize = 200;

/// The environment variable [`crate::supervisor`] sets on the server it runs.
///
/// The presence of a supervisor is not something a process can ask the kernel —
/// "my parent is a `remote-hand-server up`" is not knowable portably, and being
/// pid 1's child means nothing after a reparent. So the supervisor says so —
/// under this name and its old one, `RH_SUPERVISED`, the only one a pre-rename
/// `up` sets; such an `up` stays the parent of every server a self-upgrade
/// brings ([`crate::env`]).
pub const SUPERVISED_ENV: &str = "DROPPEN_SUPERVISED";

/// What happens to this server once the new binary is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Restart {
    /// A supervisor is watching; this process will stop and be replaced.
    Automatic,
    /// Nobody is watching; the swap is done but somebody must restart it.
    Manual,
}

impl Restart {
    /// Whether this server is running under [`crate::supervisor`].
    ///
    /// The environment marker is the whole test, and deliberately so. Living
    /// under `<data>/bin` is not evidence of a supervisor, and taking it as
    /// proof is wrong in the direction that hurts: running the installed binary
    /// by hand — the obvious thing to do when debugging — would answer
    /// `restarting`, and then the server would exit and nothing would bring it
    /// back. Being in a directory is not being watched.
    ///
    /// The marker has no matching hole, because [`crate::supervisor`] sets it on
    /// every child it starts, unconditionally.
    fn detect() -> Self {
        Self::from_marker(crate::env::var(SUPERVISED_ENV).as_deref())
    }

    /// The decision, separated from where the marker is read.
    ///
    /// Split out so it can be tested without writing to the process
    /// environment, which every other test in this binary shares.
    fn from_marker(marker: Option<&str>) -> Self {
        if marker == Some("1") {
            Self::Automatic
        } else {
            Self::Manual
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "restarting",
            Self::Manual => "manual",
        }
    }
}

/// The outcome of replacing the server binary.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SelfUpgrade {
    /// Always true; a failure is an `Error` body with a 4xx or 5xx status.
    pub ok: bool,
    /// The version the uploaded binary reports for itself.
    pub version: String,
    /// The version of the server that answered this request.
    pub previous_version: String,
    /// `restarting` when a supervisor will bring the new binary up by itself,
    /// `manual` when the swap is done but somebody has to restart the server.
    pub restart: String,
    /// Why `restart` is what it is, in a sentence meant to be shown.
    pub detail: String,
}

/// The `multipart/form-data` body this route reads.
///
/// gotcha derives nothing from a `Multipart` extractor, so the parts the handler
/// looks at are documented by hand, exactly as the file uploads do.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct ServerBinaryUpload {
    /// The new server binary. Required.
    pub binary: serde_json::Value,
    /// Lowercase hex SHA-256 of `binary`. Optional; verified when present.
    pub sha256: serde_json::Value,
}

impl AppState {
    /// The refusal a server run by the desktop app gives: `403` with a reason a
    /// person can act on, and a warning in the log saying who asked.
    fn refuse_self_upgrade_under_app(&self) -> Result<(), ApiError> {
        if !self.desktop_app {
            return Ok(());
        }
        tracing::warn!("self-upgrade refused: this server is run by the Droppen desktop app, which updates it with the app");
        Err(ApiError::with_body(
            gotcha::axum::http::StatusCode::FORBIDDEN,
            serde_json::json!({
                "error": "self_upgrade_disabled",
                "detail": "this server is run by the Droppen desktop app, which updates it with the app; update the app instead",
            }),
        ))
    }
}

/// Replace the server binary, and restart into it if something is supervising.
#[api(id = "self_upgrade", group = "meta", summary = "Replace the server binary")]
pub async fn self_upgrade(State(state): State<AppState>, multipart: Multipart) -> Result<Json<SelfUpgrade>, ApiError> {
    // Before anything else, and before a byte is read: a server the desktop app
    // runs is updated with the app, and this route would only replace the
    // shared `<data>/bin` that other instances run (`crate::desktop_app`).
    state.refuse_self_upgrade_under_app()?;
    // Ownership first, before a byte is read: replacing `<data>/bin` and
    // restarting is an owner's move, and the flock is only meaningful while
    // its file is the one on disk. A lost or absent claim means another server
    // may own this instance now — refuse, cheaply and loudly.
    // A claim that no longer upgrades belongs to a server that has stopped
    // serving, which is no owner either.
    match state.lock.get().and_then(std::sync::Weak::upgrade) {
        None => {
            return Err(ApiError::with_body(
                gotcha::axum::http::StatusCode::CONFLICT,
                serde_json::json!({
                    "error": "instance_lock_lost",
                    "detail": "this server holds no instance lock; refusing to replace the shared binary",
                }),
            ));
        }
        Some(held) => {
            if let Err(lost) = held.verify_still_held() {
                return Err(ApiError::with_body(
                    gotcha::axum::http::StatusCode::CONFLICT,
                    serde_json::json!({ "error": "instance_lock_lost", "detail": lost.to_string() }),
                ));
            }
        }
    }
    let store = BinStore::new(state.paths.as_ref());
    store
        .create()
        .map_err(|error| ApiError::internal(format!("cannot prepare {}: {error}", store.dir().display())))?;

    // Anything a previous attempt left behind goes now, before this one adds to
    // it. Cheap, and it means a crash mid-upload costs one stale file rather
    // than one per attempt forever.
    store.sweep_staging();

    // The guard comes back from `receive` rather than being built here: it has
    // to exist from the moment the file does, and half the ways this can fail
    // — a client that hangs up, a full disk, a hash that does not match — happen
    // inside `receive`, where a guard built afterwards would never run.
    let upload = store.receive_upload(multipart).await?;

    if let Some(expected) = &upload.sha256 {
        if !expected.eq_ignore_ascii_case(&upload.digest) {
            return Err(ApiError::bad_request(format!(
                "the uploaded binary hashes to {}, not the {expected} you sent",
                upload.digest
            )));
        }
    }

    let version = health_check(upload.staged.path()).await.map_err(ApiError::bad_request)?;

    store
        .promote(upload.staged.path())
        .map_err(|error| ApiError::internal(format!("cannot install the new binary: {error}")))?;
    // `promote` consumed it; there is nothing left to clean up.
    upload.staged.promoted();

    let restart = Restart::detect();
    tracing::info!(%version, restart = restart.as_str(), path = %store.installed().display(), "installed a new server binary");

    let detail = match restart {
        Restart::Automatic => {
            let cancel = state.cancel.clone();
            // Spawned rather than awaited: this handler has a response to return
            // first, and the whole point of the delay is that it happens after.
            tokio::spawn(async move {
                tokio::time::sleep(FLUSH_GRACE).await;
                tracing::info!("stopping so the supervisor can start the new binary");
                // The graceful path, not `exit(0)`: it detaches the terminals
                // and apps that are meant to outlive this process, and it exits
                // zero, which is what tells the supervisor this was deliberate.
                cancel.cancel();
            });
            "this server is supervised and will restart into the new binary in a moment; terminals, runs and apps keep running".to_string()
        }
        Restart::Manual => format!(
            "the new binary is installed at {}, but this server was not started by `remote-hand-server up` \
             and cannot restart itself. Restart it to pick the new binary up.",
            store.installed().display()
        ),
    };

    Ok(Json(SelfUpgrade {
        ok: true,
        version,
        previous_version: crate::api::SERVER_VERSION.to_string(),
        restart: restart.as_str().to_string(),
        detail,
    }))
}

/// A staged upload that is deleted unless it is promoted.
///
/// Every way out of the handler between "the bytes landed" and "the rename
/// happened" is a `return`, and each one would otherwise leave a binary-sized
/// file in the data volume. A guard makes that impossible to forget rather than
/// something to remember at six call sites.
struct Staged {
    path: std::path::PathBuf,
}

impl Staged {
    /// Where the upload landed.
    fn path(&self) -> &Path {
        &self.path
    }

    /// Give up ownership: the file is in its final place and must not be
    /// deleted.
    fn promoted(self) {
        std::mem::forget(self);
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A binary that arrived, what it hashed to, and the guard that owns it.
struct Upload {
    /// Owns the file: dropping this `Upload` on any error path deletes it.
    staged: Staged,
    digest: String,
    sha256: Option<String>,
}

impl BinStore {
    /// Stream the `binary` part to a staging file, hashing as it goes.
    ///
    /// Every failure from the `File::create` onwards carries the guard out
    /// with it, so there is no path through this function that leaves a file
    /// behind. That is worth stating because the ways out are not all visible:
    /// a client that hangs up mid-body surfaces as an error from
    /// `field.chunk()`, which is a `?` like any other.
    async fn receive_upload(&self, mut multipart: Multipart) -> Result<Upload, ApiError> {
        let path = self.staging(&format!("{}-{}", std::process::id(), uuid::Uuid::new_v4()));
        let mut upload: Option<Upload> = None;
        let mut sha256 = None;

        while let Some(mut field) = multipart
            .next_field()
            .await
            .map_err(|error| ApiError::bad_request(format!("malformed upload: {error}")))?
        {
            match field.name() {
                Some("binary") => {
                    let mut hasher = Sha256::new();
                    let mut file = tokio::fs::File::create(&path)
                        .await
                        .map_err(|error| ApiError::internal(format!("cannot write {}: {error}", path.display())))?;
                    // From here the file exists, so from here it is owned. Built
                    // before the first write rather than after the last one,
                    // because everything in between can fail.
                    let staged = Staged { path: path.clone() };
                    let mut written: u64 = 0;
                    // Streamed rather than buffered: a server binary is tens of
                    // megabytes and a debug build several times that, and holding
                    // all of it in memory to write it straight back out is the kind
                    // of thing that turns an upgrade into an OOM kill.
                    while let Some(chunk) = field
                        .chunk()
                        .await
                        .map_err(|error| ApiError::bad_request(format!("upload interrupted: {error}")))?
                    {
                        written += chunk.len() as u64;
                        // Checked here as well as by the transport's body limit, so
                        // that going over answers with something that says what
                        // happened and what the ceiling is, rather than the bare
                        // `413` a layer produces with no body at all.
                        if written > BINARY_LIMIT {
                            return Err(ApiError::new(
                                gotcha::axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                                format!("the uploaded binary is larger than the {BINARY_LIMIT}-byte limit"),
                            ));
                        }
                        hasher.update(&chunk);
                        file.write_all(&chunk)
                            .await
                            .map_err(|error| ApiError::internal(format!("cannot write {}: {error}", path.display())))?;
                    }
                    file.flush()
                        .await
                        .map_err(|error| ApiError::internal(format!("cannot write {}: {error}", path.display())))?;
                    drop(file);
                    store::make_executable(&path).map_err(|error| ApiError::internal(format!("cannot make the upload executable: {error}")))?;
                    upload = Some(Upload {
                        staged,
                        digest: hex(&hasher.finalize()),
                        sha256: None,
                    });
                }
                Some("sha256") => {
                    let text = field
                        .text()
                        .await
                        .map_err(|error| ApiError::bad_request(format!("malformed sha256 field: {error}")))?;
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        sha256 = Some(text);
                    }
                }
                _ => continue,
            }
        }

        match upload {
            // The parts may arrive in either order, so the hash is attached once
            // both have been seen rather than when the binary was.
            Some(upload) => Ok(Upload { sha256, ..upload }),
            // Nothing was staged, so nothing needs removing.
            None => Err(ApiError::bad_request("the upload has no `binary` part")),
        }
    }
}

/// Make a staged binary prove it runs here, and report the version it claims.
///
/// Two runs rather than one. `--version` proves the file is an executable this
/// kernel and architecture will actually start — the failure a cross-machine
/// upload hits first — and that it is a remote-hand server rather than any other
/// program. `preflight` proves it can also load this server's configuration,
/// which is the other way a technically-valid binary fails to come up.
async fn health_check(path: &Path) -> Result<String, String> {
    let printed = run(path, "--version").await?;
    // The *first line*, not the whole stream. A version is one short line, and
    // treating everything a program happened to print as its version is how a
    // chatty binary ends up with a megabyte of "version" in the response.
    let version = printed.lines().next().unwrap_or_default().trim();
    if version.is_empty() {
        return Err("the uploaded binary printed nothing for `--version`".to_string());
    }
    if version.len() > MAX_VERSION {
        return Err(format!(
            "the uploaded binary answered `--version` with {} characters, which is not a version",
            version.len()
        ));
    }
    run(path, "preflight").await?;
    Ok(version.to_string())
}

/// Run the staged binary with one argument, and return what it printed.
///
/// The output is *capped*, which matters more than it sounds. This runs a
/// program that has been uploaded but not yet trusted — that is the entire point
/// of the step — so "collect everything it writes" is a memory budget handed to
/// a stranger. A staged binary that `exec`s `yes` would take this process to
/// several gigabytes of RSS in a few seconds; in a memory-capped container the
/// resulting OOM kill reads to the supervisor as a fast failure, and three of
/// those quarantine the *healthy* binary that was running all along. So each pipe
/// keeps at most [`CAPTURE_CAP`] and discards the rest — discards rather than
/// stops reading, because a full pipe would block the child and turn a chatty
/// binary into a ten-second timeout instead of an immediate answer.
async fn run(path: &Path, arg: &str) -> Result<String, String> {
    let mut child = tokio::process::Command::new(path)
        .arg(arg)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("the uploaded binary could not be run (`{arg}`): {error}"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let finish = async {
        let (out, err, status) = tokio::join!(capped(stdout), capped(stderr), child.wait());
        (out, err, status)
    };

    let (out, err, status) = match tokio::time::timeout(HEALTH_TIMEOUT, finish).await {
        Ok(result) => result,
        Err(_) => return Err(format!("the uploaded binary did not answer `{arg}` within {HEALTH_TIMEOUT:?}")),
    };
    let status = status.map_err(|error| format!("the uploaded binary could not be run (`{arg}`): {error}"))?;

    if !status.success() {
        let reason = err.trim().lines().next_back().unwrap_or("no output").to_string();
        return Err(format!("the uploaded binary failed `{arg}` ({status}): {reason}"));
    }
    Ok(out)
}

/// Read a pipe to the end, keeping only the first [`CAPTURE_CAP`] bytes.
async fn capped(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> String {
    let Some(mut pipe) = pipe else {
        return String::new();
    };
    use tokio::io::AsyncReadExt;
    let mut kept: Vec<u8> = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        match pipe.read(&mut buffer).await {
            // A read error on a pipe whose child is being judged is not worth
            // reporting separately: whatever was collected so far is the answer,
            // and the exit status is what decides.
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if kept.len() < CAPTURE_CAP {
                    let room = CAPTURE_CAP - kept.len();
                    kept.extend_from_slice(&buffer[..read.min(room)]);
                }
            }
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
}

impl Paths {
    /// Seed `<data>/bin` from the running binary, so a supervisor has one to
    /// run.
    ///
    /// Called from the supervisor rather than from serving, because it is the
    /// supervisor that needs an installed binary to exist. Best effort in the
    /// sense that a failure is reported and not fatal: a supervisor with no
    /// writable data directory should still run the baked binary rather than
    /// refuse to start.
    pub fn seed_binary_store(&self, from: &Path) -> BinStore {
        let store = BinStore::new(self);
        // Before anything else: a previous boot may have died mid-upload, and a
        // staging file is the size of a server binary.
        store.sweep_staging();
        match store.seed(from) {
            Ok(true) => tracing::info!(from = %from.display(), to = %store.installed().display(), "seeded the server binary"),
            Ok(false) => {}
            Err(error) => tracing::warn!(dir = %store.dir().display(), %error, "cannot seed the server binary; running the built-in one"),
        }
        store
    }

    /// Remove any staged binary an unfinished upgrade left in `<data>/bin`.
    ///
    /// Called at boot as well as before each upgrade, because the two catch
    /// different things: the boot sweep is what clears a file left by a process
    /// that was killed outright, and the per-request one keeps a server that is
    /// never restarted from accumulating them.
    pub fn sweep_staging_binaries(&self) {
        BinStore::new(self).sweep_staging();
    }
}

/// Register this subsystem's routes.
///
/// A one-route sub-router so the body limit is this route's and not every
/// route's, exactly as the file uploads do it.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router.merge(
        ApiRouter::default()
            .post("/api/server/self-upgrade", self_upgrade)
            .layer(gotcha::axum::extract::DefaultBodyLimit::max((BINARY_LIMIT + MULTIPART_OVERHEAD) as usize)),
    )
}

/// Everything gotcha cannot derive about this operation.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe("POST", "/api/server/self-upgrade", 200, "The new binary is installed")
        .error(
            "POST",
            "/api/server/self-upgrade",
            400,
            "The upload was malformed, hashed to something else, or failed its health check",
        )
        .error("POST", "/api/server/self-upgrade", 413, "The uploaded binary is over the size limit")
        .error(
            "POST",
            "/api/server/self-upgrade",
            403,
            "This server is run by the Droppen desktop app, which updates it with the app",
        )
        .error(
            "POST",
            "/api/server/self-upgrade",
            409,
            "This server's instance lock is lost or absent; another server may own the instance",
        )
        .request_body::<ServerBinaryUpload>(
            "POST",
            "/api/server/self-upgrade",
            "multipart/form-data",
            "The replacement server binary, and optionally its SHA-256",
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_server_run_by_the_desktop_app_refuses_to_replace_the_shared_binary() {
        let home = tempfile::tempdir().expect("tempdir");
        let mut state = crate::api::test_support::state(home.path()).await;
        assert!(state.refuse_self_upgrade_under_app().is_ok(), "an ordinary server is not refused here");
        state.desktop_app = true;
        let error = state.refuse_self_upgrade_under_app().expect_err("refused under the app");
        let response = gotcha::axum::response::IntoResponse::into_response(error);
        assert_eq!(response.status(), gotcha::axum::http::StatusCode::FORBIDDEN);
        let body = gotcha::axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(body["error"], "self_upgrade_disabled");
        assert!(
            !BinStore::new(state.paths.as_ref()).dir().exists(),
            "nothing was written to the shared <data>/bin"
        );
    }

    #[test]
    fn a_digest_is_lowercase_hex() {
        assert_eq!(
            hex(&Sha256::digest(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[tokio::test]
    async fn a_program_that_is_not_a_server_never_passes_the_health_check() {
        let dir = tempfile::tempdir().expect("tempdir");

        // Not an executable at all.
        let text = dir.path().join("not-a-binary");
        std::fs::write(&text, "#!/nonexistent\n").expect("write");
        store::make_executable(&text).expect("chmod");
        assert!(health_check(&text).await.is_err());

        // A real program, but one that exits zero and says nothing: the
        // "printed output" half of the check is what catches it.
        let quiet = dir.path().join("quiet");
        std::fs::write(&quiet, "#!/bin/sh\nexit 0\n").expect("write");
        store::make_executable(&quiet).expect("chmod");
        let error = health_check(&quiet).await.expect_err("silence is not a version");
        assert!(error.contains("printed nothing"), "{error}");

        // A program that answers `--version` but not `preflight`, which is every
        // build older than this feature.
        let old = dir.path().join("old");
        std::fs::write(&old, "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 0.0.1; exit 0; fi\nexit 1\n").expect("write");
        store::make_executable(&old).expect("chmod");
        let error = health_check(&old).await.expect_err("a binary that cannot preflight is refused");
        assert!(error.contains("preflight"), "{error}");
    }

    #[tokio::test]
    async fn a_binary_that_will_not_stop_talking_is_read_in_bounded_memory() {
        // An unbounded read would let a staged binary that prints without
        // stopping decide how much memory this process uses, and in a
        // memory-capped container the OOM kill that follows looks to the
        // supervisor like a fast failure — three of which quarantine the healthy
        // binary that was running fine.
        let dir = tempfile::tempdir().expect("tempdir");
        let chatty = dir.path().join("chatty");
        // A real version on the first line, then far more than the cap. `yes`
        // is the unbounded version of this; a bounded one keeps the test quick
        // while exercising the same path.
        std::fs::write(
            &chatty,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo '1.2.3'; fi\nhead -c 4000000 /dev/zero | tr '\\0' 'x'\nexit 0\n",
        )
        .expect("write");
        store::make_executable(&chatty).expect("chmod");

        let printed = run(&chatty, "--version").await.expect("the binary exits zero");
        assert!(
            printed.len() <= CAPTURE_CAP,
            "kept {} bytes of a 4 MB stream; the cap is {CAPTURE_CAP}",
            printed.len()
        );
        // And the version read out of it is the first line, not the flood.
        assert_eq!(health_check(&chatty).await.expect("healthy"), "1.2.3");
    }

    #[tokio::test]
    async fn a_version_that_is_not_a_version_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rambling = dir.path().join("rambling");
        // One enormous line, so "take the first line" alone would not save us.
        std::fs::write(
            &rambling,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then head -c 5000 /dev/zero | tr '\\0' 'v'; echo; fi\nexit 0\n",
        )
        .expect("write");
        store::make_executable(&rambling).expect("chmod");
        let error = health_check(&rambling).await.expect_err("5000 characters is not a version");
        assert!(error.contains("not a version"), "{error}");
    }

    #[test]
    fn only_the_supervisor_s_own_marker_means_supervised() {
        // Living under `<data>/bin` is not proof of a supervisor: taken as
        // proof, running the installed binary by hand would answer
        // `restarting`, exit, and nothing would bring it back.
        assert_eq!(Restart::from_marker(None), Restart::Manual);
        assert_eq!(Restart::from_marker(Some("1")), Restart::Automatic);
        assert_eq!(Restart::from_marker(Some("0")), Restart::Manual, "only an explicit 1 counts");
        assert_eq!(Restart::from_marker(Some("")), Restart::Manual);
    }

    #[tokio::test]
    async fn a_binary_that_answers_both_questions_reports_its_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let good = dir.path().join("good");
        std::fs::write(
            &good,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo '9.9.9+gdeadbee'; fi\nexit 0\n",
        )
        .expect("write");
        store::make_executable(&good).expect("chmod");
        assert_eq!(health_check(&good).await.expect("healthy"), "9.9.9+gdeadbee");
    }

    #[test]
    fn a_staged_upload_is_deleted_unless_it_is_promoted() {
        let root = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(root.path().to_path_buf());
        let store = BinStore::new(&paths);
        store.create().expect("create");

        let path = store.staging("abandoned");
        std::fs::write(&path, "x").expect("write");
        drop(Staged { path: path.clone() });
        assert!(!path.exists(), "a refused upload leaves nothing behind");

        let path = store.staging("kept");
        std::fs::write(&path, "x").expect("write");
        let staged = Staged { path: path.clone() };
        staged.promoted();
        assert!(path.exists(), "a promoted upload is the caller's to rename");
    }
}
