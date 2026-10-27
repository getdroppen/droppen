//! Pictures an agent links to that the browser cannot fetch by itself.
//!
//! An agent that opens a pull request against a private repository puts its
//! screenshots *in* that repository (a `pr-assets` branch, say) and writes
//! `![…](https://github.com/<owner>/<repo>/blob/<ref>/<path>?raw=true)` into
//! its reply. GitHub answers that URL with a `404` to anyone without a session
//! cookie for github.com, so the transcript shows a broken image to everyone
//! who is not, at that moment, logged into GitHub in the same browser.
//!
//! `GET /api/media/github?url=…` fetches it instead, through the host's own
//! `gh` CLI — the same login the agent pushed the file with. The server never
//! sees the token: `gh` reads it from its own keyring, makes the request, and
//! hands back bytes on stdout.
//!
//! # Why this does not widen anyone's access
//!
//! Every Droppen user can already run agents with full shell access on this
//! host as the same OS user, and `gh api …` is one shell command away for any
//! of them. This route exposes strictly less than that: one read-only endpoint
//! (the contents API, `GET` only), on `github.com` only, answering only with
//! bytes that sniff as an image. It is not a general repository reader and not
//! a general fetcher — see [`parse_github_url`] for the URL shapes it accepts,
//! and everything else is a `400` before a process is started.
//!
//! # Refs with slashes
//!
//! `blob/feature/x/docs/a.png` does not say where the ref ends and the path
//! begins; GitHub's own web UI resolves it against the repository's ref list.
//! This tries the contents API with the shortest ref first and lengthens it by
//! one segment on each `404`, up to [`MAX_REF_SEGMENTS`]. The common case — a
//! one-segment branch, a tag or a sha — costs exactly one call; only a miss
//! pays for the rest, and a miss is a broken picture either way.

use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use gotcha::axum::body::{Body, Bytes};
use gotcha::axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS};
use gotcha::axum::http::StatusCode;
use gotcha::axum::response::Response;
use gotcha::{api, Schematic};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

use crate::api::error::ApiError;
use crate::api::extract::ApiQuery;
use crate::api::ApiRouter;
use crate::files::{FileBytes, INLINE_CSP};
use crate::openapi_ext::ExtraResponses;
use crate::process_env::{apply_env, clean_env_bare, resolve_on_path, Env};

/// The route, written once for the router, the spec and the tests.
const ROUTE: &str = "/api/media/github";
/// How long one `gh` call may take, start to exit.
const GH_TIMEOUT: Duration = Duration::from_secs(20);
/// The largest image served; a longer stdout is cut off and the child killed.
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
/// How much of `gh`'s stderr is kept to classify a failure.
const STDERR_CAP: u64 = 8 * 1024;
/// How many ref segments [`fetch_image`] will try before giving up.
const MAX_REF_SEGMENTS: usize = 4;
/// How many `gh` processes may run at once, server-wide. A transcript with
/// thirty screenshots should queue thirty calls, not fork thirty processes.
const GH_CONCURRENCY: usize = 4;
/// How long the browser may reuse an answer. An hour rather than a day: an
/// agent that re-pushes a screenshot under the same name on its assets branch
/// is the normal case, and a stale picture of a fixed bug is worse than a fetch.
const BROWSER_CACHE: &str = "private, max-age=3600";
/// How long a fetched image is served from memory without asking GitHub again.
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
/// Ceilings on that memory: a handful of screenshots, not a mirror.
const CACHE_MAX_ENTRIES: usize = 64;
const CACHE_MAX_BYTES: usize = 48 * 1024 * 1024;

/// What `gh` says, verbatim enough for the web's fallback card to show.
const GH_MISSING: &str = "the gh CLI is not installed on the server";
const GH_LOGGED_OUT: &str = "gh is not logged in on the server (run `gh auth login`)";
const GH_REJECTED: &str = "gh's GitHub login was rejected (run `gh auth login` again)";

// ---------------------------------------------------------------------------
// Wire shapes
// ---------------------------------------------------------------------------

/// Query for `GET /api/media/github`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct GithubMediaQuery {
    /// The image URL exactly as the agent wrote it: a `github.com` `blob`/`raw`
    /// URL or a `raw.githubusercontent.com` one.
    pub url: String,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// A file in a GitHub repository, named by a URL this route accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubTarget {
    /// The account or organisation.
    pub owner: String,
    /// The repository.
    pub repo: String,
    /// The ref and the path, percent-decoded, still together: which segments
    /// are the ref is not knowable from the URL (see the module docs).
    pub rest: Vec<String>,
}

impl GithubTarget {
    /// Every (ref, path) split worth trying, shortest ref first.
    ///
    /// A first segment of `refs` is a full ref name (`refs/heads/main`, which
    /// `raw.githubusercontent.com` URLs now carry), so the splits that would
    /// name `refs` or `refs/heads` alone are skipped rather than spent.
    pub fn candidates(&self) -> Vec<(String, String)> {
        let full_ref = self.rest.len() > 3 && self.rest[0] == "refs" && matches!(self.rest[1].as_str(), "heads" | "tags");
        let first = if full_ref { 3 } else { 1 };
        let last = (self.rest.len() - 1).min(if full_ref { MAX_REF_SEGMENTS + 2 } else { MAX_REF_SEGMENTS });
        let mut out = Vec::new();
        for split in first..=last {
            // A segment that cannot be part of a ref makes every longer ref
            // invalid too, so the walk stops at the first one.
            if !valid_ref_segment(&self.rest[split - 1]) {
                break;
            }
            out.push((self.rest[..split].join("/"), self.rest[split..].join("/")));
        }
        out
    }

    /// The cache key: owner and repository case-folded, as GitHub treats them.
    fn key(&self) -> String {
        format!(
            "{}/{}/{}",
            self.owner.to_ascii_lowercase(),
            self.repo.to_ascii_lowercase(),
            self.rest.join("/")
        )
    }
}

/// Parse an image URL into the file it names, or say why it is refused.
///
/// Accepted, and nothing else:
///
/// * `https://github.com/<owner>/<repo>/blob/<ref>/<path>`, bare or with
///   `?raw=true` / `?raw=1`;
/// * `https://github.com/<owner>/<repo>/raw/<ref>/<path>`, likewise;
/// * `https://raw.githubusercontent.com/<owner>/<repo>/<ref>/<path>`, bare or
///   with the `?token=…` a private raw link carries — which is dropped, since
///   `gh` brings its own credential.
///
/// A `#fragment` is ignored (READMEs use `#gh-dark-mode-only`). The host is
/// compared whole, so `github.com.evil.com`, a port or userinfo are refused;
/// every segment is percent-decoded and must not be empty, `.`, `..`, or hide
/// a `/` or `\` behind an escape, so nothing can climb out of the contents API.
pub fn parse_github_url(raw: &str) -> Result<GithubTarget, &'static str> {
    const NOT_GITHUB: &str = "not a GitHub image URL";
    if raw.bytes().any(|b| b.is_ascii_control() || b == b' ') {
        return Err(NOT_GITHUB);
    }
    let rest = strip_prefix_ignore_case(raw, "https://").ok_or(NOT_GITHUB)?;
    let rest = rest.split_once('#').map_or(rest, |(before, _)| before);
    let (authority, rest) = rest.split_once('/').ok_or(NOT_GITHUB)?;
    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (rest, None),
    };

    let mut segments = Vec::new();
    for segment in path.split('/') {
        segments.push(decode_segment(segment).ok_or("invalid path segment")?);
    }

    let raw_host = authority.eq_ignore_ascii_case("raw.githubusercontent.com");
    if !raw_host && !authority.eq_ignore_ascii_case("github.com") {
        return Err(NOT_GITHUB);
    }
    let (owner, repo, tail) = match segments.as_slice() {
        [owner, repo, tail @ ..] => (owner, repo, tail),
        _ => return Err(NOT_GITHUB),
    };
    let rest = if raw_host {
        if !query.is_none_or(|query| only_param(query, "token", |_| true)) {
            return Err("unexpected query parameters");
        }
        tail
    } else {
        match tail {
            [kind, rest @ ..] if kind == "blob" || kind == "raw" => {
                if !query.is_none_or(|query| only_param(query, "raw", |value| value == "true" || value == "1")) {
                    return Err("unexpected query parameters");
                }
                rest
            }
            _ => return Err(NOT_GITHUB),
        }
    };

    if !valid_owner(owner) || !valid_repo(repo) {
        return Err("invalid owner or repository");
    }
    // At least one segment of ref and one of path.
    if rest.len() < 2 || !valid_ref_segment(&rest[0]) {
        return Err("missing ref or path");
    }
    Ok(GithubTarget {
        owner: owner.clone(),
        repo: repo.clone(),
        rest: rest.to_vec(),
    })
}

fn strip_prefix_ignore_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let head = value.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &value[prefix.len()..])
}

/// Whether a query string is exactly one `name=value` pair whose value passes.
fn only_param(query: &str, name: &str, accept: impl Fn(&str) -> bool) -> bool {
    if query.is_empty() {
        return true;
    }
    match query.split_once('=') {
        Some((key, value)) => key == name && !value.contains('&') && accept(value),
        None => false,
    }
}

/// One path segment, percent-decoded, or `None` if it is unusable.
fn decode_segment(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = segment.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(out).ok()?;
    let usable = !decoded.is_empty() && decoded != "." && decoded != ".." && !decoded.contains(['/', '\\']) && !decoded.chars().any(char::is_control);
    usable.then_some(decoded)
}

/// GitHub logins: ASCII letters, digits and hyphens, at most 39 of them.
fn valid_owner(owner: &str) -> bool {
    (1..=39).contains(&owner.len()) && owner.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// GitHub repository names: ASCII letters, digits, `-`, `_` and `.`.
fn valid_repo(repo: &str) -> bool {
    (1..=100).contains(&repo.len()) && repo != "." && repo != ".." && repo.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// The parts of `git check-ref-format` a single segment can break.
fn valid_ref_segment(segment: &str) -> bool {
    !segment.starts_with('.')
        && !segment.ends_with(".lock")
        && !segment.contains("..")
        && !segment.contains("@{")
        && !segment.chars().any(|c| c.is_whitespace() || "~^:?*[\\".contains(c))
}

/// Percent-escape everything outside RFC 3986's unreserved set.
fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(byte) {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

impl GithubTarget {
    /// The contents-API endpoint for one (ref, path) split, relative to the API root.
    ///
    /// Every path segment and the whole ref are re-encoded from their decoded form,
    /// so the ref's slashes travel as `%2F` inside the query and nothing the URL
    /// carried can become a `?`, `&`, `#` or `{owner}` placeholder for `gh`.
    pub fn contents_endpoint(&self, git_ref: &str, path: &str) -> String {
        let path = path.split('/').map(encode_component).collect::<Vec<_>>().join("/");
        format!("repos/{}/{}/contents/{path}?ref={}", self.owner, self.repo, encode_component(git_ref))
    }
}


/// The full argv after `gh`. Passed as a vector to the process — there is no
/// shell anywhere in between, so nothing in the endpoint is interpreted twice.
pub fn gh_args(endpoint: &str) -> Vec<String> {
    [
        "api",
        "--hostname",
        "github.com",
        "--method",
        "GET",
        "-H",
        "Accept: application/vnd.github.raw",
        endpoint,
    ]
    .map(str::to_string)
    .to_vec()
}

// ---------------------------------------------------------------------------
// Image sniffing
// ---------------------------------------------------------------------------

/// The image type the bytes really are, or `None`.
///
/// The extension in the URL is not trusted: the content type comes from the
/// bytes, and only these types are served at all.
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if is_avif(bytes) {
        return Some("image/avif");
    }
    if is_svg(bytes) {
        return Some("image/svg+xml");
    }
    None
}

/// An ISO-BMFF `ftyp` box whose major or a compatible brand is AVIF.
fn is_avif(bytes: &[u8]) -> bool {
    if bytes.len() < 16 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    let size = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let end = size.clamp(16, 256).min(bytes.len());
    let brand = |at: usize| matches!(&bytes[at..at + 4], b"avif" | b"avis");
    // Major brand at 8, minor version at 12, compatible brands from 16.
    brand(8) || (16..end).step_by(4).any(|at| at + 4 <= end && brand(at))
}

/// A document whose first element is `<svg`, after an optional BOM, XML
/// declaration, comments and doctype.
///
/// Served as `image/svg+xml` under [`INLINE_CSP`] — a sandbox with scripts off
/// and no network — so even opened as a document it can only draw itself.
fn is_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(4096)];
    let text = String::from_utf8_lossy(head);
    let mut rest = text.trim_start_matches('\u{feff}').trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("<?") {
            let Some((_, tail)) = after.split_once("?>") else { return false };
            rest = tail.trim_start();
        } else if let Some(after) = rest.strip_prefix("<!--") {
            let Some((_, tail)) = after.split_once("-->") else { return false };
            rest = tail.trim_start();
        } else if rest.get(..9).is_some_and(|head| head.eq_ignore_ascii_case("<!doctype")) {
            let Some((_, tail)) = rest.split_once('>') else { return false };
            rest = tail.trim_start();
        } else {
            break;
        }
    }
    rest.strip_prefix("<svg")
        .and_then(|after| after.chars().next())
        .is_some_and(|c| c.is_whitespace() || c == '>' || c == '/')
}

// ---------------------------------------------------------------------------
// Fetching
// ---------------------------------------------------------------------------

/// Why a fetch failed, already reduced to what the client may be told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    /// No such file at any ref the URL could name, or no access to it.
    NotFound,
    /// `gh` answered, but with something that is not an image.
    NotAnImage,
    /// The image is larger than [`MAX_IMAGE_BYTES`].
    TooLarge,
    /// `gh` did not finish within [`GH_TIMEOUT`].
    Timeout,
    /// `gh` is missing or not logged in; the message says which.
    Unavailable(&'static str),
    /// Anything else, as a short reason of our own — never `gh`'s stderr.
    Upstream(String),
}

impl MediaError {
    fn into_api(self) -> ApiError {
        match self {
            MediaError::NotFound => ApiError::not_found("image not found on GitHub"),
            MediaError::NotAnImage => ApiError::new(StatusCode::UNSUPPORTED_MEDIA_TYPE, "the file is not an image"),
            MediaError::TooLarge => ApiError::new(StatusCode::BAD_GATEWAY, "the image is larger than 20 MiB"),
            MediaError::Timeout => ApiError::new(StatusCode::GATEWAY_TIMEOUT, "gh did not answer in time"),
            MediaError::Unavailable(message) => ApiError::new(StatusCode::SERVICE_UNAVAILABLE, message),
            MediaError::Upstream(message) => ApiError::new(StatusCode::BAD_GATEWAY, message),
        }
    }
}

/// One contents-API call. A seam, so the ref walk and the sniffing can be
/// tested without a network, a `gh` or a GitHub account.
pub trait Fetch {
    /// The raw bytes at `endpoint`, or why not.
    fn fetch(&self, endpoint: &str) -> impl Future<Output = Result<Bytes, MediaError>> + Send;
}

/// A fetched image and its sniffed content type.
#[derive(Debug, Clone)]
pub struct Image {
    /// The image's bytes.
    pub bytes: Bytes,
    /// What [`sniff_image`] made of them.
    pub mime: &'static str,
}

impl GithubTarget {
    /// Walk the (ref, path) splits until one exists, then prove it is an image.
    pub async fn fetch_image<F: Fetch>(&self, fetcher: &F) -> Result<Image, MediaError> {
        for (git_ref, path) in self.candidates() {
            match fetcher.fetch(&self.contents_endpoint(&git_ref, &path)).await {
                Ok(bytes) => {
                    let mime = sniff_image(&bytes).ok_or(MediaError::NotAnImage)?;
                    return Ok(Image { bytes, mime });
                }
                Err(MediaError::NotFound) => continue,
                Err(other) => return Err(other),
            }
        }
        Err(MediaError::NotFound)
    }
}


/// What a failed `gh api` run means, from its exit code and stderr.
///
/// `gh` prints `gh: <message> (HTTP <status>)` for an HTTP failure and exits 4
/// when it has no credential at all. Only the status is read; the message is
/// GitHub's and stays in the server log.
pub fn classify_failure(code: Option<i32>, stderr: &str) -> MediaError {
    if code == Some(4) {
        return MediaError::Unavailable(GH_LOGGED_OUT);
    }
    let status = stderr
        .rsplit_once("(HTTP ")
        .and_then(|(_, tail)| tail.get(..3))
        .and_then(|digits| digits.parse::<u16>().ok());
    match status {
        Some(404) => MediaError::NotFound,
        Some(401) => MediaError::Unavailable(GH_REJECTED),
        Some(status) => MediaError::Upstream(format!("GitHub answered HTTP {status}")),
        None if stderr.contains("gh auth login") => MediaError::Unavailable(GH_LOGGED_OUT),
        None => MediaError::Upstream("gh failed".to_string()),
    }
}

/// The real fetcher: the host's `gh`, as a child process.
struct GhCli {
    program: PathBuf,
    env: Env,
}

/// Server-wide cap on concurrent `gh` processes; see [`GH_CONCURRENCY`].
static GH_SLOTS: LazyLock<tokio::sync::Semaphore> = LazyLock::new(|| tokio::sync::Semaphore::new(GH_CONCURRENCY));

impl GhCli {
    /// `gh` from the server's `PATH`, or the `503` that says it is missing.
    fn locate() -> Result<Self, MediaError> {
        let mut env = clean_env_bare();
        // Never wait on a prompt or an update check: there is no terminal.
        env.insert("GH_PROMPT_DISABLED".to_string(), "1".to_string());
        env.insert("GH_NO_UPDATE_NOTIFIER".to_string(), "1".to_string());
        env.insert("NO_COLOR".to_string(), "1".to_string());
        let program = resolve_on_path("gh", &env).ok_or(MediaError::Unavailable(GH_MISSING))?;
        Ok(Self { program, env })
    }

    async fn run(&self, endpoint: &str) -> Result<Bytes, MediaError> {
        let _slot = GH_SLOTS.acquire().await.map_err(|_| MediaError::Upstream("gh failed".to_string()))?;
        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(gh_args(endpoint))
            // Neutral ground: `gh` reads the repository it is run inside for
            // defaults, and this request has nothing to do with any of them.
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        apply_env(&mut command, &self.env);
        let mut child = command.spawn().map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => MediaError::Unavailable(GH_MISSING),
            _ => MediaError::Upstream("could not start gh".to_string()),
        })?;
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            return Err(MediaError::Upstream("gh failed".to_string()));
        };

        let collected = tokio::time::timeout(GH_TIMEOUT, async {
            let mut out = Vec::new();
            let mut err = Vec::new();
            // One byte past the cap is how an oversized image is told apart
            // from one exactly at it; reading stops there either way.
            let mut stdout = stdout.take(MAX_IMAGE_BYTES + 1);
            let mut stderr = stderr.take(STDERR_CAP);
            let reading = async {
                let read = stdout.read_to_end(&mut out).await;
                // Killed here rather than after the join: a child blocked on a
                // full stdout pipe never closes stderr, so waiting for both
                // would turn every oversized image into a timeout.
                if out.len() as u64 > MAX_IMAGE_BYTES {
                    let _ = child.start_kill();
                }
                read
            };
            let (read_out, _) = tokio::join!(reading, stderr.read_to_end(&mut err));
            read_out.map_err(|_| MediaError::Upstream("gh failed".to_string()))?;
            if out.len() as u64 > MAX_IMAGE_BYTES {
                return Err(MediaError::TooLarge);
            }
            let status = child.wait().await.map_err(|_| MediaError::Upstream("gh failed".to_string()))?;
            Ok((status, out, err))
        })
        .await;

        let (status, out, err) = match collected {
            Ok(Ok(done)) => done,
            Ok(Err(error)) => {
                let _ = child.kill().await;
                return Err(error);
            }
            Err(_) => {
                let _ = child.kill().await;
                return Err(MediaError::Timeout);
            }
        };
        if status.success() {
            return Ok(Bytes::from(out));
        }
        let stderr = String::from_utf8_lossy(&err);
        tracing::debug!(endpoint, stderr = %stderr.lines().next().unwrap_or_default(), "gh api failed");
        Err(classify_failure(status.code(), &stderr))
    }
}

impl Fetch for GhCli {
    fn fetch(&self, endpoint: &str) -> impl Future<Output = Result<Bytes, MediaError>> + Send {
        self.run(endpoint)
    }
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

/// A few recently fetched images, so the thumbnail and the lightbox — or two
/// tabs on the same transcript — do not each start a `gh`.
///
/// Small and short-lived on purpose: content at a branch moves, and this is a
/// courtesy, not a mirror. Least-recently-used goes first.
#[derive(Default)]
struct ImageCache {
    entries: VecDeque<(String, Image, Instant)>,
}

impl ImageCache {
    fn get(&mut self, key: &str, now: Instant) -> Option<Image> {
        self.entries.retain(|(_, _, at)| now.duration_since(*at) < CACHE_TTL);
        let index = self.entries.iter().position(|(k, _, _)| k == key)?;
        let entry = self.entries.remove(index)?;
        let image = entry.1.clone();
        self.entries.push_back(entry);
        Some(image)
    }

    fn put(&mut self, key: String, image: Image, now: Instant) {
        if image.bytes.len() > CACHE_MAX_BYTES {
            return;
        }
        self.entries.retain(|(k, _, _)| *k != key);
        self.entries.push_back((key, image, now));
        let mut total: usize = self.entries.iter().map(|(_, image, _)| image.bytes.len()).sum();
        while self.entries.len() > CACHE_MAX_ENTRIES || total > CACHE_MAX_BYTES {
            let Some((_, evicted, _)) = self.entries.pop_front() else { break };
            total -= evicted.bytes.len();
        }
    }
}

static CACHE: LazyLock<Mutex<ImageCache>> = LazyLock::new(Mutex::default);

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// An image from a GitHub repository, fetched with the host's `gh` login.
#[api(id = "media_github", group = "media", summary = "Proxy a GitHub image")]
pub async fn github_image(ApiQuery(query): ApiQuery<GithubMediaQuery>) -> Result<FileBytes, ApiError> {
    let target = parse_github_url(&query.url).map_err(ApiError::bad_request)?;
    let key = target.key();
    let cached = CACHE.lock().expect("media cache poisoned").get(&key, Instant::now());
    let image = match cached {
        Some(image) => image,
        None => {
            let gh = GhCli::locate().map_err(MediaError::into_api)?;
            let image = target.fetch_image(&gh).await.map_err(MediaError::into_api)?;
            CACHE.lock().expect("media cache poisoned").put(key, image.clone(), Instant::now());
            image
        }
    };

    let response = Response::builder()
        .header(CONTENT_TYPE, image.mime)
        // The sniffed type is the only type; see `files::fs_download`.
        .header(X_CONTENT_TYPE_OPTIONS, "nosniff")
        // An SVG opened on its own still runs nothing and reaches nothing.
        .header(CONTENT_SECURITY_POLICY, INLINE_CSP)
        .header(CACHE_CONTROL, BROWSER_CACHE)
        .body(Body::from(image.bytes))
        .map_err(|_| ApiError::internal("cannot build response"))?;
    Ok(FileBytes(response))
}

/// Register this subsystem's routes.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router.get(ROUTE, github_image)
}

/// Everything gotcha cannot derive about this route.
pub fn describe(reg: &mut ExtraResponses) {
    reg.binary("GET", ROUTE, 200, "The image's bytes, fetched with the host's gh CLI")
        .error("GET", ROUTE, 400, "Not a GitHub image URL this proxy accepts")
        .error("GET", ROUTE, 404, "No such file at any ref the URL could name")
        .error("GET", ROUTE, 415, "The file is not an image")
        .error("GET", ROUTE, 502, "gh or GitHub failed, or the image is larger than 20 MiB")
        .error("GET", ROUTE, 503, "gh is not installed or not logged in on the server")
        .error("GET", ROUTE, 504, "gh did not answer within 20 seconds");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn parse(url: &str) -> GithubTarget {
        parse_github_url(url).unwrap_or_else(|error| panic!("{url} was refused: {error}"))
    }

    fn parts(target: &GithubTarget) -> (String, String, String) {
        (target.owner.clone(), target.repo.clone(), target.rest.join("/"))
    }

    #[test]
    fn the_shapes_an_agent_writes_are_accepted() {
        let blob = parse("https://github.com/Octo/example-app/blob/pr-assets/pr-47/sidebar-mac-light.png?raw=true");
        assert_eq!(
            parts(&blob),
            ("Octo".into(), "example-app".into(), "pr-assets/pr-47/sidebar-mac-light.png".into())
        );
        for url in [
            "https://github.com/o/r/blob/main/a.png",
            "https://github.com/o/r/blob/main/a.png?raw=1",
            "https://github.com/o/r/raw/main/a.png",
            "https://github.com/o/r/raw/main/a.png?raw=true",
            "HTTPS://GitHub.com/o/r/blob/main/a.png",
            "https://raw.githubusercontent.com/o/r/main/a.png",
            "https://github.com/o/r/blob/main/a.png#gh-dark-mode-only",
        ] {
            assert_eq!(parts(&parse(url)), ("o".into(), "r".into(), "main/a.png".into()), "{url}");
        }
    }

    #[test]
    fn a_raw_token_is_accepted_and_dropped() {
        let target = parse("https://raw.githubusercontent.com/o/r/main/docs/a.png?token=GHSAT0AAAA");
        assert_eq!(target.rest, ["main", "docs", "a.png"]);
        let endpoint = target.contents_endpoint( "main", "docs/a.png");
        assert!(!endpoint.contains("token") && !endpoint.contains("GHSAT"), "{endpoint}");
    }

    #[test]
    fn segments_are_percent_decoded_once() {
        let target = parse("https://github.com/o/r/blob/main/%E6%88%AA%E5%9B%BE%20one.png");
        assert_eq!(target.rest, ["main", "截图 one.png"]);
    }

    #[test]
    fn other_hosts_and_lookalikes_are_refused() {
        for url in [
            "https://github.com.evil.com/o/r/blob/main/a.png",
            "https://evil.com/github.com/o/r/blob/main/a.png",
            "https://evilgithub.com/o/r/blob/main/a.png",
            "https://github.com:443/o/r/blob/main/a.png",
            "https://user@github.com/o/r/blob/main/a.png",
            "https://api.github.com/repos/o/r/contents/a.png",
            "https://gist.github.com/o/abc/raw/a.png",
            "http://github.com/o/r/blob/main/a.png",
            "file:///etc/passwd",
            "//github.com/o/r/blob/main/a.png",
            "github.com/o/r/blob/main/a.png",
            "https://github.com",
            "",
        ] {
            assert!(parse_github_url(url).is_err(), "{url} was accepted");
        }
    }

    #[test]
    fn traversal_and_hidden_separators_are_refused() {
        for url in [
            "https://github.com/o/r/blob/main/../../../etc/passwd",
            "https://github.com/o/r/blob/main/./a.png",
            "https://github.com/o/r/blob/main/%2e%2e/a.png",
            "https://github.com/o/r/blob/main/a%2Fb.png",
            "https://github.com/o/r/blob/main/a%5Cb.png",
            "https://github.com/o/r/blob/main%2Fx/a.png",
            "https://github.com/o/r/blob/main//a.png",
            "https://github.com/o/r/blob/main/a.png/",
            "https://github.com/o/r/blob/main/a%00.png",
            "https://github.com/o/r/blob/main/a%zz.png",
            "https://github.com/o/r/blob/main/a%E6.png",
            "https://github.com/../r/blob/main/a.png",
            "https://github.com/o/r/blob/main/a b.png",
            "https://github.com/o/r/blob/main/a.png\n",
        ] {
            assert!(parse_github_url(url).is_err(), "{url} was accepted");
        }
    }

    #[test]
    fn other_github_pages_and_query_junk_are_refused() {
        for url in [
            "https://github.com/o/r/tree/main/docs",
            "https://github.com/o/r/blob/main",
            "https://github.com/o/r",
            "https://github.com/user-attachments/assets/6d0b4c1e-0000-4000-8000-000000000000",
            "https://github.com/o/r/assets/1234/6d0b4c1e-0000-4000-8000-000000000000",
            "https://github.com/o/r/blob/main/a.png?raw=false",
            "https://github.com/o/r/blob/main/a.png?raw=true&x=1",
            "https://github.com/o/r/blob/main/a.png?x=1",
            "https://github.com/o/r/blob/main/a.png?token=abc",
            "https://raw.githubusercontent.com/o/r/main/a.png?raw=true",
            "https://raw.githubusercontent.com/o/r/main/a.png?token=a&b=c",
            "https://github.com/o_o/r/blob/main/a.png",
            "https://github.com/o/r%20x/blob/main/a.png",
            "https://github.com/o/r/blob/.hidden/a.png",
            "https://github.com/o/r/blob/a..b/a.png",
            "https://github.com/o/r/blob/x~1/a.png",
        ] {
            assert!(parse_github_url(url).is_err(), "{url} was accepted");
        }
    }

    #[test]
    fn refs_with_slashes_are_tried_shortest_first() {
        let target = parse("https://github.com/o/r/blob/feature/login/docs/a.png");
        assert_eq!(
            target.candidates(),
            [
                ("feature".to_string(), "login/docs/a.png".to_string()),
                ("feature/login".to_string(), "docs/a.png".to_string()),
                ("feature/login/docs".to_string(), "a.png".to_string()),
            ]
        );
        // A full ref name skips the splits that could only name `refs`.
        let full = parse("https://raw.githubusercontent.com/o/r/refs/heads/main/a.png");
        assert_eq!(full.candidates(), [("refs/heads/main".to_string(), "a.png".to_string())]);
        // The walk is capped, and stops at a segment no ref could contain.
        let deep = parse("https://github.com/o/r/blob/a/b/c/d/e/f/g.png");
        assert_eq!(deep.candidates().len(), MAX_REF_SEGMENTS);
        let odd = parse("https://github.com/o/r/blob/main/x:y/a.png");
        assert_eq!(odd.candidates(), [("main".to_string(), "x:y/a.png".to_string())]);
    }

    #[test]
    fn the_endpoint_reencodes_everything_it_carries() {
        let target = parse("https://github.com/o/r/blob/main/%E6%88%AA%E5%9B%BE%3F%23%7Bowner%7D.png");
        let (git_ref, path) = &target.candidates()[0];
        assert_eq!(
            target.contents_endpoint( git_ref, path),
            "repos/o/r/contents/%E6%88%AA%E5%9B%BE%3F%23%7Bowner%7D.png?ref=main"
        );
        assert_eq!(
            target.contents_endpoint( "feature/login", "docs/a b.png"),
            "repos/o/r/contents/docs/a%20b.png?ref=feature%2Flogin"
        );
    }

    #[test]
    fn the_argv_is_a_fixed_vector_with_the_endpoint_last() {
        let endpoint = "repos/o/r/contents/a.png?ref=main";
        assert_eq!(
            gh_args(endpoint),
            [
                "api",
                "--hostname",
                "github.com",
                "--method",
                "GET",
                "-H",
                "Accept: application/vnd.github.raw",
                endpoint
            ]
        );
        // An endpoint always starts with `repos/`, so it can never be taken
        // for a flag, and `;`/`$()` in it are just bytes in one argument.
        let hostile = parse("https://github.com/o/r/blob/main/%3B%20rm%20-rf%20%24(id).png");
        let (git_ref, path) = &hostile.candidates()[0];
        let args = gh_args(&hostile.contents_endpoint( git_ref, path));
        assert_eq!(args.len(), 8);
        assert!(args[7].starts_with("repos/") && !args[7].contains(' ') && !args[7].contains(';'));
    }

    #[test]
    fn images_are_sniffed_from_their_bytes() {
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), Some("image/png"));
        assert_eq!(sniff_image(b"\xff\xd8\xff\xe0\0\x10JFIF"), Some("image/jpeg"));
        assert_eq!(sniff_image(b"GIF89a\x01\0"), Some("image/gif"));
        assert_eq!(sniff_image(b"RIFF\x24\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_image(b"\0\0\0\x1cftypavif\0\0\0\0avifmif1"), Some("image/avif"));
        assert_eq!(sniff_image(b"\0\0\0\x20ftypmif1\0\0\0\0mif1avifmiaf\0\0\0\0"), Some("image/avif"));
        assert_eq!(sniff_image(b"\0\0\0\x18ftypisom\0\0\0\0isommp41"), None, "an mp4 is not an image");
        assert_eq!(sniff_image(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"), Some("image/svg+xml"));
        assert_eq!(
            sniff_image(b"\xef\xbb\xbf<?xml version=\"1.0\"?>\n<!-- x -->\n<!DOCTYPE svg>\n<svg>"),
            Some("image/svg+xml")
        );
        assert_eq!(sniff_image(b"<html><svg></svg></html>"), None, "an HTML page is not an SVG");
        assert_eq!(sniff_image(b"<svgfoo>"), None);
        assert_eq!(sniff_image(b"{\"message\":\"Not Found\"}"), None);
        assert_eq!(sniff_image(b"version https://git-lfs.github.com/spec/v1\n"), None, "an LFS pointer");
        assert_eq!(sniff_image(b""), None);
    }

    #[test]
    fn gh_failures_map_to_short_reasons() {
        assert_eq!(classify_failure(Some(1), "gh: Not Found (HTTP 404)\n"), MediaError::NotFound);
        assert_eq!(
            classify_failure(Some(1), "gh: No commit found for the ref pr-assets/pr-47 (HTTP 404)\n"),
            MediaError::NotFound
        );
        assert_eq!(
            classify_failure(Some(1), "gh: Bad credentials (HTTP 401)"),
            MediaError::Unavailable(GH_REJECTED)
        );
        assert_eq!(
            classify_failure(Some(4), "To get started with GitHub CLI, please run:  gh auth login"),
            MediaError::Unavailable(GH_LOGGED_OUT)
        );
        assert_eq!(
            classify_failure(Some(1), "gh: Resource protected by organization SAML enforcement (HTTP 403)"),
            MediaError::Upstream("GitHub answered HTTP 403".into()),
            "GitHub's own sentence stays in the log"
        );
        assert_eq!(
            classify_failure(Some(1), "error connecting to api.github.com"),
            MediaError::Upstream("gh failed".into())
        );
        let api = MediaError::Unavailable(GH_MISSING).into_api();
        assert_eq!(api.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(api.body()["error"], GH_MISSING);
    }

    /// Answers from a table, and remembers what it was asked.
    struct Table {
        answers: HashMap<String, Result<Bytes, MediaError>>,
        asked: Mutex<Vec<String>>,
    }

    impl Table {
        fn new(answers: &[(&str, Result<&'static [u8], MediaError>)]) -> Self {
            Self {
                answers: answers
                    .iter()
                    .map(|(endpoint, answer)| (endpoint.to_string(), answer.clone().map(Bytes::from_static)))
                    .collect(),
                asked: Mutex::new(Vec::new()),
            }
        }
    }

    impl Fetch for Table {
        fn fetch(&self, endpoint: &str) -> impl Future<Output = Result<Bytes, MediaError>> + Send {
            self.asked.lock().unwrap().push(endpoint.to_string());
            let answer = self.answers.get(endpoint).cloned().unwrap_or(Err(MediaError::NotFound));
            async move { answer }
        }
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    #[tokio::test]
    async fn a_slash_ref_is_found_by_lengthening_it() {
        let target = parse("https://github.com/o/r/blob/feature/login/a.png");
        let table = Table::new(&[("repos/o/r/contents/a.png?ref=feature%2Flogin", Ok(PNG))]);
        let image = target.fetch_image(&table).await.expect("found");
        assert_eq!(image.mime, "image/png");
        assert_eq!(
            *table.asked.lock().unwrap(),
            [
                "repos/o/r/contents/login/a.png?ref=feature",
                "repos/o/r/contents/a.png?ref=feature%2Flogin"
            ]
        );
    }

    #[tokio::test]
    async fn the_common_case_costs_one_call() {
        let target = parse("https://github.com/Octo/example-app/blob/pr-assets/pr-47/sidebar-mac-light.png?raw=true");
        let table = Table::new(&[(
            "repos/Octo/example-app/contents/pr-47/sidebar-mac-light.png?ref=pr-assets",
            Ok(PNG),
        )]);
        assert!(target.fetch_image(&table).await.is_ok());
        assert_eq!(table.asked.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_non_image_or_a_hard_failure_stops_the_walk() {
        let target = parse("https://github.com/o/r/blob/main/a/b.png");
        let text = Table::new(&[("repos/o/r/contents/a/b.png?ref=main", Ok(b"<!doctype html><p>hi"))]);
        assert_eq!(target.fetch_image(&text).await.unwrap_err(), MediaError::NotAnImage);

        let down = Table::new(&[("repos/o/r/contents/a/b.png?ref=main", Err(MediaError::Unavailable(GH_LOGGED_OUT)))]);
        assert_eq!(target.fetch_image(&down).await.unwrap_err(), MediaError::Unavailable(GH_LOGGED_OUT));
        assert_eq!(down.asked.lock().unwrap().len(), 1, "no point trying a longer ref without a login");

        let missing = Table::new(&[]);
        assert_eq!(target.fetch_image(&missing).await.unwrap_err(), MediaError::NotFound);
        assert_eq!(missing.asked.lock().unwrap().len(), 2);
    }

    #[test]
    fn the_cache_expires_and_evicts_least_recently_used() {
        let image = |n: usize| Image {
            bytes: Bytes::from(vec![0u8; n]),
            mime: "image/png",
        };
        let start = Instant::now();
        let mut cache = ImageCache::default();
        cache.put("a".into(), image(1), start);
        assert!(cache.get("a", start + CACHE_TTL / 2).is_some());
        assert!(cache.get("a", start + CACHE_TTL).is_none(), "expired");

        let mut cache = ImageCache::default();
        for n in 0..CACHE_MAX_ENTRIES {
            cache.put(format!("k{n}"), image(1), start);
        }
        assert!(cache.get("k0", start).is_some(), "touching k0 makes k1 the oldest");
        cache.put("new".into(), image(1), start);
        assert!(cache.get("k0", start).is_some());
        assert!(cache.get("k1", start).is_none());

        let mut cache = ImageCache::default();
        cache.put("big1".into(), image(CACHE_MAX_BYTES / 2 + 1), start);
        cache.put("big2".into(), image(CACHE_MAX_BYTES / 2 + 1), start);
        assert!(cache.get("big1", start).is_none(), "evicted for size");
        assert!(cache.get("big2", start).is_some());
    }
}
