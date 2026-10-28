//! Browsing, reading, downloading and uploading files under the home directory.
//!
//! Every path the client names is resolved and then proven to sit under the home
//! directory before a syscall touches it, and the two upload routes are the only
//! writes the API performs outside its own config directory. A device reaching
//! this computer through Droppen is fenced out of the server's own data and
//! config directories on top of that, save what the transcript links to in
//! there ([`fence`]).
//!
//! Every entry point takes the request's [`Paths`], and nothing here may read
//! `$HOME` or `dirs::home_dir()` directly: the integration tests point a whole
//! server at a temporary home, and a direct read would step out of it.

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use gotcha::axum::body::Body;
use gotcha::axum::http::header::{CONTENT_DISPOSITION, CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS};
use gotcha::axum::http::StatusCode;
use gotcha::axum::response::{IntoResponse, Response};
use gotcha::{api, Json, Multipart, Schematic, State};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

use crate::api::error::ApiError;
use crate::api::extract::ApiQuery;
use crate::api::{ApiRouter, AppState};
use crate::auth::MaybeIdentity;
use crate::openapi_ext::{ExtraResponses, Nullable};
use crate::sessions::{iso8601, Paths};

/// How much of a file `GET /api/fs/read` will return.
const READ_LIMIT: u64 = 256 * 1024;
/// Ceiling on `POST /api/fs/upload`.
const UPLOAD_LIMIT: u64 = 200 * 1024 * 1024;
/// Ceiling on `POST /api/uploads`, lower because attachments ride along with a message.
const ATTACHMENT_LIMIT: u64 = 100 * 1024 * 1024;
/// Headroom over the file-size ceiling for the multipart framing that carries it.
///
/// axum caps every request body at 2 MiB unless a route says otherwise
/// (`DefaultBodyLimit`); a route that keeps the cap refuses a larger body with
/// a bare `413` before the handler runs. Each upload route raises its cap to
/// its ceiling plus this, so a file *at* the ceiling still arrives, and one
/// just over it is refused by the handler's own check, with its message. A body
/// past even the raised cap fails while the multipart body is being read, which
/// the handler reports as its fallback text (a `400`), not a bare `413`.
const MULTIPART_OVERHEAD: u64 = 64 * 1024;
/// How long an uploaded-but-never-sent attachment survives; see [`sweep_uploads`].
const UPLOAD_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How much of a download is read per stream poll.
const DOWNLOAD_CHUNK: usize = 64 * 1024;

/// The message every containment failure carries.
const OUTSIDE_HOME: &str = "path is outside the home directory";
/// What a device reaching this computer through Droppen is told about the
/// server's own directories ([`fence`]).
const PRIVATE_TO_THIS_COMPUTER: &str = "this device's Droppen data and settings directories are not reachable from another device";

/// Extensions the download route is willing to serve inline, and their mime types.
///
/// Narrower than the allowlist behind
/// [`is_image_ext`](crate::sessions::util::is_image_ext) — this list decides
/// what the browser is invited to render in place rather than save — but note
/// what it still contains: an
/// `svg` served as `image/svg+xml` with `Content-Disposition: inline` is a
/// *document*, and a script inside it would otherwise run on this server's
/// origin — where the page keeps its bearer token in the origin's own storage,
/// and an agent that can write a file can write an SVG.
///
/// [`INLINE_CSP`] defuses it rather than dropping `svg` from this list: an
/// avatar or a diagram in a repository is exactly the thing the file browser
/// exists to preview, and the sandbox costs nothing to display it.
const INLINE_IMAGE_MIME: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("svg", "image/svg+xml"),
];

/// What every inline response carries, so that "render this in place" cannot
/// mean "run this in place".
///
/// `sandbox` with no allow-list is the whole of it: the browser gives the
/// resource a unique opaque origin and switches scripting off, which is exactly
/// the two capabilities a malicious SVG needs — reaching this origin's storage
/// for a session token, and calling the API with it. An `<img>` renders
/// identically either way, because an image was never going to use them.
///
/// `default-src 'none'` goes with it so a sandboxed document cannot phone home
/// with what it can still see (an external `<image href>` inside the SVG is a
/// beacon for "this person previewed this file").
pub(crate) const INLINE_CSP: &str = "sandbox; default-src 'none'; style-src 'unsafe-inline'; img-src data:";

// ---------------------------------------------------------------------------
// Wire shapes
// ---------------------------------------------------------------------------

/// What a directory entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsKind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// A symbolic link, reported as itself rather than followed.
    Symlink,
    /// A socket, fifo, device — anything with no browsing meaning.
    Other,
}

// Inline in `FsEntry`: the contract spells `kind` as an inline string enum,
// not a named component.
crate::inline_string_enum!(FsKind, ["file", "dir", "symlink", "other"]);

/// One row of a directory listing.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct FsEntry {
    /// The entry's own name, not a path.
    pub name: String,
    /// What the entry is.
    pub kind: FsKind,
    /// Size in bytes; only regular files carry one, and it is null when the stat failed.
    pub size_bytes: Nullable<i64>,
    /// Modification time, ISO-8601; filled on the same terms as `sizeBytes`.
    pub mtime: Nullable<String>,
}

/// A directory listing.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct FsList {
    /// The resolved absolute path that was listed.
    pub path: String,
    /// The parent directory, or null at the home directory — the browser's ceiling.
    pub parent: Nullable<String>,
    /// Directories first, then names in ascending order.
    pub entries: Vec<FsEntry>,
}

/// A file's text, up to the read cap.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct FsRead {
    /// The resolved absolute path that was read.
    pub path: String,
    /// The file's full size, which may exceed the returned slice.
    pub size_bytes: i64,
    /// Modification time, ISO-8601.
    pub mtime: Nullable<String>,
    /// True when the slice held a NUL byte, in which case `content` is empty.
    pub binary: bool,
    /// True when the file is larger than the 256 KiB read cap.
    pub truncated: bool,
    /// The decoded slice, or an empty string for a binary file.
    pub content: String,
}

/// Where a file landed after `POST /api/fs/upload`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct FsUpload {
    /// Absolute path of the written file.
    pub path: String,
    /// How many bytes were written.
    pub size_bytes: i64,
}

/// A file parked in the uploads directory, ready to attach to a message.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// Absolute path, always under [`Paths::uploads_dir`].
    pub path: String,
    /// How many bytes were written.
    pub size_bytes: i64,
    /// The original filename, preserved so the download name means something.
    pub name: String,
    /// Whether the composer should draw a thumbnail for it.
    pub is_image: bool,
}

/// Query for `GET /api/fs/list`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct FsListQuery {
    /// Directory to list; the home directory when absent.
    pub path: Option<String>,
}

/// Query for `GET /api/fs/read`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct FsReadQuery {
    /// File to read.
    pub path: String,
}

/// Query for `GET /api/fs/download`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct FsDownloadQuery {
    /// File to download.
    pub path: String,
    /// Serve inline with an image content type, for `<img>` thumbnails.
    pub inline: Option<bool>,
}

/// Query for `POST /api/fs/upload`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct FsUploadQuery {
    /// Directory to save into; it must already exist.
    pub dir: String,
}

/// The raw file body `GET /api/fs/download` streams.
///
/// gotcha documents an operation from its return type, and the blanket
/// `Responsible` impl needs the type to be `Schematic`
/// (`gotcha_core-0.4.1/src/responsible.rs:81`), which `axum::response::Response`
/// is not. Leaving the handler un-annotated is not a way out either: only
/// annotated handlers are recorded as operations at all
/// (`gotcha-0.4.1/src/router.rs:104`), so the route would vanish from the spec
/// rather than merely losing its response schema — and `openapi_ext::apply`
/// would then log its override as targeting an unregistered route.
///
/// So the response travels inside a newtype whose `Schematic` is hand-written
/// (it registers no component, exactly as `openapi_ext::AnyValue` does), and
/// [`describe`] replaces the JSON `200` that yields with the
/// `application/octet-stream` entry the contract names.
pub struct FileBytes(pub(crate) Response);

impl IntoResponse for FileBytes {
    fn into_response(self) -> Response {
        self.0
    }
}

impl Schematic for FileBytes {
    fn name() -> &'static str {
        "FileBytes"
    }

    fn required() -> bool {
        true
    }

    fn type_() -> &'static str {
        "string"
    }

    fn format() -> Option<String> {
        Some("binary".to_string())
    }
}

// ---------------------------------------------------------------------------
// Containment
// ---------------------------------------------------------------------------

/// A filesystem failure, split by what the client is told.
///
/// A refusal the user can act on carries its own message; any other failure
/// (a failed syscall) is answered with the route's fallback text, never with
/// the OS error.
#[derive(Debug)]
pub enum FsError {
    /// A refusal the user should read: its message is what the client sees.
    Access(&'static str),
    /// A device reaching this computer through Droppen asked for the
    /// server's own data or settings directory ([`fence`]): a `403`.
    Private,
    /// Anything else, replaced by the route's fallback text.
    Other,
}

impl FsError {
    /// Render as a `400`, substituting `fallback` for [`FsError::Other`]
    /// (and a `403 not_from_cloud` for [`FsError::Private`]).
    pub fn into_api(self, fallback: &'static str) -> ApiError {
        match self {
            FsError::Access(message) => ApiError::bad_request(message),
            FsError::Private => ApiError::with_body(
                StatusCode::FORBIDDEN,
                serde_json::json!({"error": "not_from_cloud", "message": PRIVATE_TO_THIS_COMPUTER}),
            ),
            FsError::Other => ApiError::bad_request(fallback),
        }
    }
}

/// Who a file route serves: the people at this computer, or a device that
/// reaches it through Droppen (`cloud:<device>`, bridged by the agent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// A local user (or nobody signed in, with local authentication off).
    Local,
    /// A device of the Droppen account, over an end-to-end channel.
    Cloud,
}

impl Reach {
    /// The reach of a request's identity.
    pub fn of(who: &MaybeIdentity) -> Self {
        match &who.0 {
            Some(identity) if identity.username.starts_with("cloud:") => Reach::Cloud,
            _ => Reach::Local,
        }
    }
}

/// What a file route does with the path it is fenced on ([`fence`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// List, read or download.
    Read,
    /// Upload into it.
    Write,
}

/// The managed Claude root's directories a device reaching this computer
/// through Droppen may read, though they sit in the server's data directory
/// ([`fence`]): what the transcript links to there.
///
/// * `plans` — a session in plan mode writes its plan to
///   `$CLAUDE_CONFIG_DIR/plans/<name>.md` and says so; the path becomes a
///   download chip on that reply ([`crate::sessions::mentions`]) or a link
///   in its markdown.
/// * `image-cache` — where Claude Code keeps an image pasted into a session,
///   whose path it hands the model; a `Read` of it is offered as the tool
///   result's thumbnail ([`crate::sessions::transcript`]).
///
/// Plus [`READABLE_IN_PROJECTS`]. Nothing else in the root is linked from
/// anywhere — its transcripts reach the page through the session routes,
/// never through these — and the rest of it is a login (`.credentials.json`,
/// `.claude.json` and its `backups/`), what was pasted (`paste-cache`), file
/// backups, shell snapshots and settings.
const READABLE_IN_CLAUDE_ROOT: [&str; 2] = ["plans", "image-cache"];

/// Directories under the managed root's `projects/` a device may read too,
/// by name and how many levels under `projects/` they sit:
///
/// * `<project>/memory` — the auto-memory: the notes a session saves and
///   names (`MEMORY.md` and one file per memory), which become chips on the
///   reply and links in its markdown.
/// * `<project>/<session>/tool-results` — a tool's output too large for the
///   transcript, which Claude Code saves there and names to the model, which
///   quotes the path (a chip) or reads it back (a thumbnail when it is an
///   image).
///
/// Read-only to devices, like [`READABLE_IN_CLAUDE_ROOT`].
const READABLE_IN_PROJECTS: [(&str, usize); 2] = [("memory", 2), ("tool-results", 3)];

/// Defence in depth for devices that reach this computer through Droppen: the
/// server's own data directory (the instance's machine key, chain state,
/// accounts, the managed Claude root…) and its config directory are not theirs
/// to list, read, download or write into —
/// whichever path leads there, through `..` (gone by [`resolve_safe`]) or
/// symlinks, dangling ones included, and whatever the case on a
/// case-insensitive disk (directories are compared by identity, not by
/// name).
///
/// What the transcript shows from in there stays reachable: the uploads
/// directory (composer attachments), and — to read only — this
/// instance's managed Claude root's plans, pasted images, memory notes and
/// saved tool output ([`READABLE_IN_CLAUDE_ROOT`], [`READABLE_IN_PROJECTS`]),
/// judged on where
/// the path really lands, so a symlink in one of them leads nowhere else.
/// Another instance's root is not this instance's to open.
///
/// A directory walk cannot see a hard link, so one in a reopened directory
/// (the uploads directory included) is refused too: a regular file there
/// with more than one name may be a login or a key from elsewhere in the
/// fenced trees under a name the walk lets through. Claude Code and this
/// server only ever write single-named files there. Elsewhere in the home
/// directory a hard-linked file reads like any other — a build's outputs
/// (cargo links every one), a local `git clone`'s objects, a pnpm store on a
/// disk without clones — because no name there is in the fenced trees, and a
/// second name made for a fenced file outside them is a copy that no
/// path-based fence can tell apart from any other.
///
/// Local users are not fenced. Not a boundary — a device that may control
/// this computer can run a terminal on it — but no file route hands these
/// out by itself.
pub fn fence(paths: &Paths, reach: Reach, access: Access, target: &Path) -> Result<(), FsError> {
    judge(paths, reach, access, target).map(drop)
}

/// How a path the fence let through got through ([`fence`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Through {
    /// None of the server's own directories was in its way (or the request
    /// is local, and nothing is fenced).
    Open,
    /// It is in one of them, and goes through only because a directory there
    /// is reopened: the uploads directory, or one of
    /// [`READABLE_IN_CLAUDE_ROOT`] and [`READABLE_IN_PROJECTS`]. A listing
    /// of such a directory leaves out what a read of it would refuse.
    Reopened,
}

/// [`fence`], saying how the path got through.
fn judge(paths: &Paths, reach: Reach, access: Access, target: &Path) -> Result<Through, FsError> {
    if reach == Reach::Local {
        return Ok(Through::Open);
    }
    let real = real_path(target).ok_or(FsError::Private)?;
    let private: Vec<FileId> = [paths.data_dir(), paths.config_dir()].iter().filter_map(|d| file_id(d)).collect();
    let uploads = file_id(&paths.uploads_dir());
    let claude = paths.claude_root(crate::claude_roots::ClaudeRoot::Managed);
    let readable: Vec<FileId> = READABLE_IN_CLAUDE_ROOT.iter().filter_map(|d| real_dir_id(&claude.join(d))).collect();
    let projects = real_dir_id(&claude.join("projects"));
    for dir in real.ancestors() {
        let Some(id) = file_id(dir) else { continue };
        if uploads.as_ref() == Some(&id) {
            return single_named(&real);
        }
        if readable.contains(&id) || projects.as_ref().is_some_and(|projects| readable_in_projects(dir, &id, projects)) {
            return match access {
                Access::Read => single_named(&real),
                Access::Write => Err(FsError::Private),
            };
        }
        if private.contains(&id) {
            return Err(FsError::Private);
        }
    }
    Ok(Through::Open)
}

/// `real`, in a reopened directory, is let through unless it is a regular
/// file with another name — a hard link — which may be anything in the
/// fenced trees ([`fence`]). What is not there yet (an upload's target) is
/// for the route to find missing.
fn single_named(real: &Path) -> Result<Through, FsError> {
    match std::fs::metadata(real) {
        Ok(meta) if hard_linked(&meta) => Err(FsError::Private),
        _ => Ok(Through::Reopened),
    }
}

/// Whether `meta` is a regular file with more than one name.
#[cfg(unix)]
fn hard_linked(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.is_file() && meta.nlink() > 1
}

#[cfg(not(unix))]
fn hard_linked(_meta: &std::fs::Metadata) -> bool {
    false
}

/// Whether `dir` (identity `id`) is one of [`READABLE_IN_PROJECTS`]: the
/// entry of that name — by identity, so in whatever case the disk takes it —
/// exactly that many levels under the projects directory `projects`.
fn readable_in_projects(dir: &Path, id: &FileId, projects: &FileId) -> bool {
    let Some(parent) = dir.parent() else { return false };
    READABLE_IN_PROJECTS.iter().any(|(name, depth)| {
        real_dir_id(&parent.join(name)).as_ref() == Some(id) && dir.ancestors().nth(*depth).and_then(file_id).as_ref() == Some(projects)
    })
}

/// The identity of `path` when it is a directory itself, not a symlink to
/// one: a directory the fence opens is one Claude Code made, and a link in
/// its place does not move the opening somewhere else.
fn real_dir_id(path: &Path) -> Option<FileId> {
    std::fs::symlink_metadata(path).ok().filter(|m| m.is_dir()).and_then(|_| file_id(path))
}

/// What a directory is, whatever path names it: device and inode.
#[cfg(unix)]
type FileId = (u64, u64);
#[cfg(not(unix))]
type FileId = PathBuf;

#[cfg(unix)]
fn file_id(path: &Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

#[cfg(not(unix))]
fn file_id(path: &Path) -> Option<FileId> {
    std::fs::canonicalize(path).ok()
}

/// `path` (absolute) with every symlink in it resolved — dangling ones too,
/// so a write through one is judged where it would land — and `..` after a
/// link taken as the kernel takes it. `None` past 40 links (the kernel's
/// own limit), or for a link that cannot be read.
fn real_path(path: &Path) -> Option<PathBuf> {
    enum Part {
        Root,
        Up,
        Name(std::ffi::OsString),
    }
    fn push(stack: &mut Vec<Part>, path: &Path) {
        for component in path.components().rev() {
            match component {
                Component::Prefix(_) | Component::CurDir => {}
                Component::RootDir => stack.push(Part::Root),
                Component::ParentDir => stack.push(Part::Up),
                Component::Normal(name) => stack.push(Part::Name(name.to_os_string())),
            }
        }
    }
    let mut stack = Vec::new();
    push(&mut stack, path);
    let mut resolved = PathBuf::from(std::path::MAIN_SEPARATOR_STR);
    let mut links = 0;
    while let Some(part) = stack.pop() {
        match part {
            Part::Root => resolved = PathBuf::from(std::path::MAIN_SEPARATOR_STR),
            Part::Up => {
                resolved.pop();
            }
            Part::Name(name) => {
                let next = resolved.join(&name);
                match std::fs::symlink_metadata(&next) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        links += 1;
                        if links > 40 {
                            return None;
                        }
                        let target = std::fs::read_link(&next).ok()?;
                        push(&mut stack, &target);
                    }
                    _ => resolved = next,
                }
            }
        }
    }
    Some(resolved)
}

/// `requested` made absolute against the process working directory, with `.`
/// and `..` collapsed **textually**.
///
/// It does *not* consult the filesystem, so it does not resolve symlinks: a
/// symlink inside the home directory that points outside it passes the
/// containment check in [`resolve_safe`] and its target is served. That is
/// deliberate (no [`std::fs::canonicalize`]): the check guards against *typed*
/// paths, not against a home directory whose owner has linked something in.
/// A device reaching this computer through Droppen is judged on the real path
/// too, but only against the server's own directories ([`fence`]).
/// `tests/files.rs` pins it.
fn resolve(requested: &Path) -> PathBuf {
    let absolute = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from(std::path::MAIN_SEPARATOR_STR))
            .join(requested)
    };

    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            // `..` at the root is the root, as the kernel resolves it.
            Component::ParentDir => {
                if out.parent().is_some() {
                    out.pop();
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

/// Expand a leading `~` and prove the result stays inside the home directory.
///
/// `Path::starts_with` compares whole components, so the home directory itself
/// passes and a sibling such as `/home/alice2` does not start with `/home/alice`.
pub fn resolve_safe(paths: &Paths, requested: Option<&str>) -> Result<PathBuf, FsError> {
    let home = paths.home.as_path();
    let raw = requested.unwrap_or_default().trim();

    let expanded = if raw == "~" {
        home.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        // String concatenation, not `Path::join`: `join` treats an absolute
        // right-hand side as a replacement, so `~//etc` would become `/etc` and
        // be refused, where a shell reads it as `$HOME/etc`.
        let mut joined = home.as_os_str().to_os_string();
        joined.push(std::path::MAIN_SEPARATOR_STR);
        joined.push(rest);
        PathBuf::from(joined)
    } else if raw.is_empty() {
        home.to_path_buf()
    } else {
        PathBuf::from(raw)
    };

    let target = resolve(&expanded);
    if !target.starts_with(home) {
        return Err(FsError::Access(OUTSIDE_HOME));
    }
    Ok(target)
}

/// Prove that `requested` sits **under** the uploads directory, or fail.
///
/// This is a security gate rather than a convenience: the paths a client
/// attaches to a message are handed to the CLI as arguments (`--add-dir`, image
/// arguments), so a path that escaped the uploads directory would hand the CLI
/// read access to anything the user can read. Callers must therefore use the
/// returned, resolved path rather than the one they passed in.
///
/// The failure is the send routes' `409`; they are its only callers.
///
/// Resolution is lexical, as in [`resolve`]: a symlink inside the uploads
/// directory is accepted on the strength of its path. Only this server writes
/// into that directory, so nothing can plant one without already having
/// filesystem access.
pub fn assert_under_uploads(paths: &Paths, requested: &Path) -> Result<PathBuf, ApiError> {
    let uploads = paths.uploads_dir();
    let target = resolve(requested);
    // `starts_with` is true for the directory itself, which is no attachment.
    if target == uploads || !target.starts_with(&uploads) {
        return Err(ApiError::new(StatusCode::CONFLICT, "attachment paths must come from /api/uploads"));
    }
    Ok(target)
}

// ---------------------------------------------------------------------------
// Listing and reading
// ---------------------------------------------------------------------------

async fn list_dir(paths: &Paths, reach: Reach, requested: Option<&str>) -> Result<FsList, FsError> {
    let path = resolve_safe(paths, requested)?;
    let through = judge(paths, reach, Access::Read, &path)?;

    let mut reader = tokio::fs::read_dir(&path).await.map_err(|_| FsError::Other)?;
    let mut entries: Vec<FsEntry> = Vec::new();
    while let Some(dirent) = reader.next_entry().await.map_err(|_| FsError::Other)? {
        let name = dirent.file_name().to_string_lossy().into_owned();
        // `file_type` comes from the directory entry, so a symlink reports as a
        // symlink instead of as whatever it points at.
        let kind = match dirent.file_type().await {
            Ok(file_type) if file_type.is_dir() => FsKind::Dir,
            Ok(file_type) if file_type.is_file() => FsKind::File,
            Ok(file_type) if file_type.is_symlink() => FsKind::Symlink,
            _ => FsKind::Other,
        };

        let mut size_bytes = None;
        let mut mtime = None;
        // Only regular files are stat'ed: a directory's size means nothing here,
        // and stat'ing a symlink would follow it off the listing's own tree.
        if kind == FsKind::File {
            if let Ok(metadata) = tokio::fs::metadata(path.join(&name)).await {
                // A hard link in a reopened directory, which a device may not
                // read, is not listed to it either — nor its size and mtime,
                // which are the fenced file's ([`fence`]).
                if through == Through::Reopened && hard_linked(&metadata) {
                    continue;
                }
                size_bytes = Some(metadata.len() as i64);
                mtime = metadata.modified().ok().map(iso8601);
            }
        }

        entries.push(FsEntry {
            name,
            kind,
            size_bytes: size_bytes.into(),
            mtime: mtime.into(),
        });
    }

    // Directories first, then names in code-point order (`Ord` on `String`).
    entries.sort_by(|a, b| match ((a.kind == FsKind::Dir), (b.kind == FsKind::Dir)) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });

    // The home directory is the browser's ceiling, so it reports no parent.
    let parent = if path == paths.home {
        None
    } else {
        Some(path.parent().unwrap_or(path.as_path()).to_string_lossy().into_owned())
    };

    Ok(FsList {
        path: path.to_string_lossy().into_owned(),
        parent: parent.into(),
        entries,
    })
}

async fn read_file_safe(paths: &Paths, reach: Reach, requested: &str) -> Result<FsRead, FsError> {
    let path = resolve_safe(paths, Some(requested))?;
    fence(paths, reach, Access::Read, &path)?;
    let metadata = tokio::fs::metadata(&path).await.map_err(|_| FsError::Other)?;
    if !metadata.is_file() {
        return Err(FsError::Access("not a regular file"));
    }

    let size = metadata.len();
    let mut buffer = vec![0u8; size.min(READ_LIMIT) as usize];
    let mut file = tokio::fs::File::open(&path).await.map_err(|_| FsError::Other)?;
    // A short read is not an error: the file may have been truncated between the
    // stat and the read, which is exactly why the size is taken from the stat.
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]).await {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(_) => return Err(FsError::Other),
        }
    }
    buffer.truncate(filled);

    // A NUL byte anywhere in the slice is the sniff for "do not try to show this
    // as text" — the same heuristic `git` uses.
    let binary = buffer.contains(&0);
    Ok(FsRead {
        path: path.to_string_lossy().into_owned(),
        size_bytes: size as i64,
        mtime: metadata.modified().ok().map(iso8601).into(),
        binary,
        truncated: size > READ_LIMIT,
        content: if binary {
            String::new()
        } else {
            String::from_utf8_lossy(&buffer).into_owned()
        },
    })
}

/// The resolved path of a file to download, and its base name.
async fn stat_for_download(paths: &Paths, reach: Reach, requested: &str) -> Result<(PathBuf, String), FsError> {
    let path = resolve_safe(paths, Some(requested))?;
    fence(paths, reach, Access::Read, &path)?;
    let metadata = tokio::fs::metadata(&path).await.map_err(|_| FsError::Other)?;
    if !metadata.is_file() {
        return Err(FsError::Access("not a regular file"));
    }
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    Ok((path, name))
}

/// The file's bytes as a stream, so the response is chunked.
///
/// `unfold` rather than `tokio_util::io::ReaderStream` only because
/// `Cargo.toml` does not ask for `tokio-util`'s `io` feature.
fn file_stream(file: tokio::fs::File) -> impl futures_util::Stream<Item = std::io::Result<Vec<u8>>> {
    futures_util::stream::unfold(Some(file), |state| async move {
        let mut file = state?;
        let mut chunk = vec![0u8; DOWNLOAD_CHUNK];
        match file.read(&mut chunk).await {
            Ok(0) => None,
            Ok(read) => {
                chunk.truncate(read);
                Some((Ok(chunk), Some(file)))
            }
            // Surface the failure in the body and stop; the status line is long gone.
            Err(error) => Some((Err(error), None)),
        }
    })
}

/// `encodeURIComponent`: percent-escape every byte outside the unreserved set.
fn encode_uri_component(value: &str) -> String {
    /// The punctuation `encodeURIComponent` leaves alone, alongside `A-Za-z0-9`.
    const KEPT: &[u8] = b"-_.!~*'()";
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || KEPT.contains(byte) {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The lowercased text after a name's last `.`, or the whole name when it has none.
fn extension(name: &str) -> String {
    name.rsplit('.').next().unwrap_or_default().to_lowercase()
}

// ---------------------------------------------------------------------------
// Uploads
// ---------------------------------------------------------------------------

/// One multipart `file` field, read into memory.
///
/// Buffered rather than streamed to disk because both limits are checked against
/// the complete size before anything is written; a streamed write would have to
/// be undone on overrun.
struct UploadedFile {
    /// The client-supplied filename, unsanitised.
    name: String,
    /// The field's bytes.
    bytes: Vec<u8>,
}

/// Take the `file` field out of a multipart body.
///
/// A missing field is [`FsError::Other`], so the route answers with its
/// fallback text.
async fn take_file_field(mut multipart: Multipart) -> Result<UploadedFile, FsError> {
    while let Some(field) = multipart.next_field().await.map_err(|_| FsError::Other)? {
        if field.name() != Some("file") {
            continue;
        }
        // The name has to be copied out before `bytes` consumes the field.
        let name = field.file_name().unwrap_or_default().to_string();
        let bytes = field.bytes().await.map_err(|_| FsError::Other)?;
        return Ok(UploadedFile { name, bytes: bytes.to_vec() });
    }
    Err(FsError::Other)
}

/// The last `/`-separated segment of a name, trailing slashes ignored.
fn base_name(raw: &str) -> &str {
    let trimmed = raw.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some((_, last)) => last,
        None => trimmed,
    }
}

/// [`base_name`] with every separator stripped.
///
/// The backslash pass matters: [`base_name`] splits on `/` only, so a Windows
/// client's `C:\Users\me\x.txt` would otherwise survive whole into a filename.
fn sanitize_name(raw: &str) -> String {
    base_name(raw).chars().filter(|c| *c != '/' && *c != '\\').collect()
}

async fn save_upload(paths: &Paths, reach: Reach, dir_requested: &str, file: &UploadedFile) -> Result<FsUpload, FsError> {
    let dir = resolve_safe(paths, Some(dir_requested))?;
    fence(paths, reach, Access::Write, &dir)?;
    let metadata = tokio::fs::metadata(&dir).await.map_err(|_| FsError::Other)?;
    if !metadata.is_dir() {
        return Err(FsError::Access("not a directory"));
    }
    let size = file.bytes.len() as u64;
    if size > UPLOAD_LIMIT {
        return Err(FsError::Access("file exceeds the 200 MiB upload limit"));
    }

    let name = sanitize_name(&file.name);
    if name.is_empty() || name == "." || name == ".." {
        return Err(FsError::Access("invalid file name"));
    }
    let target = dir.join(&name);
    // The file itself too: a dangling symlink of that name would carry the
    // write wherever it points.
    fence(paths, reach, Access::Write, &target)?;

    // Refuse rather than overwrite: this writes into the user's own tree, where
    // a clobbered file has no undo. The client renames and retries.
    if tokio::fs::metadata(&target).await.is_ok() {
        return Err(FsError::Access("file already exists"));
    }

    tokio::fs::write(&target, &file.bytes).await.map_err(|_| FsError::Other)?;
    Ok(FsUpload {
        path: target.to_string_lossy().into_owned(),
        size_bytes: size as i64,
    })
}

/// Each upload lands in its own uuid directory under the original filename, so
/// the download name stays meaningful and same-named files never collide, while
/// the path still resolves under the uploads directory — which is what
/// [`assert_under_uploads`] later relies on.
async fn save_attachment(paths: &Paths, file: &UploadedFile) -> Result<Attachment, FsError> {
    let size = file.bytes.len() as u64;
    if size > ATTACHMENT_LIMIT {
        return Err(FsError::Access("file exceeds the 100 MiB upload limit"));
    }

    let trimmed = sanitize_name(&file.name).trim().to_string();
    // An attachment with no usable name still has usable bytes, so it gets one.
    let name = if trimmed.is_empty() { "file".to_string() } else { trimmed };
    if name == "." || name == ".." {
        return Err(FsError::Access("invalid file name"));
    }

    let dir = paths.uploads_dir().join(uuid::Uuid::new_v4().to_string());
    tokio::fs::create_dir_all(&dir).await.map_err(|_| FsError::Other)?;
    let target = dir.join(&name);
    tokio::fs::write(&target, &file.bytes).await.map_err(|_| FsError::Other)?;

    // This route's own extraction: `extension()` is `split('.').last()`, so a
    // name with no dot is its own extension (and "png" is an image here).
    let extension = extension(&name);
    Ok(Attachment {
        path: target.to_string_lossy().into_owned(),
        size_bytes: size as i64,
        is_image: crate::sessions::util::is_image_ext(&extension),
        name,
    })
}

/// Delete upload directories that have not been touched for seven days.
///
/// Attachments are uploaded before a message is sent, and a message that is
/// never sent leaves its files behind forever; nothing else ever collects them.
/// Failures are ignored throughout — a sweep that cannot run is not a reason to
/// refuse to serve, and the next one will try again.
pub async fn sweep_uploads(paths: &Paths) {
    let Ok(mut reader) = tokio::fs::read_dir(paths.uploads_dir()).await else {
        // No uploads directory yet: nothing to sweep.
        return;
    };

    let now = SystemTime::now();
    while let Ok(Some(entry)) = reader.next_entry().await {
        let target = entry.path();
        // An unreadable entry is skipped, not a reason to abandon the sweep.
        let Ok(metadata) = tokio::fs::metadata(&target).await else { continue };
        let Ok(modified) = metadata.modified() else { continue };
        if now.duration_since(modified).unwrap_or_default() > UPLOAD_TTL {
            let _ = tokio::fs::remove_dir_all(&target).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// List a directory under the home directory.
#[api(id = "fs_list", group = "files", summary = "List a directory")]
pub async fn fs_list(
    State(state): State<AppState>,
    #[api(skip)] who: MaybeIdentity,
    ApiQuery(query): ApiQuery<FsListQuery>,
) -> Result<Json<FsList>, ApiError> {
    list_dir(state.paths.as_ref(), Reach::of(&who), query.path.as_deref())
        .await
        .map(Json)
        .map_err(|error| error.into_api("cannot list directory"))
}

/// Read a text file under the home directory (capped at 256 KiB).
#[api(id = "fs_read", group = "files", summary = "Read a file")]
pub async fn fs_read(
    State(state): State<AppState>,
    #[api(skip)] who: MaybeIdentity,
    ApiQuery(query): ApiQuery<FsReadQuery>,
) -> Result<Json<FsRead>, ApiError> {
    read_file_safe(state.paths.as_ref(), Reach::of(&who), &query.path)
        .await
        .map(Json)
        .map_err(|error| error.into_api("cannot read file"))
}

/// Raw file bytes, as an attachment or inline for an image.
#[api(id = "fs_download", group = "files", summary = "Download a file")]
pub async fn fs_download(
    State(state): State<AppState>,
    #[api(skip)] who: MaybeIdentity,
    ApiQuery(query): ApiQuery<FsDownloadQuery>,
) -> Result<FileBytes, ApiError> {
    let fallback = "cannot download file";
    let (path, name) = stat_for_download(state.paths.as_ref(), Reach::of(&who), &query.path)
        .await
        .map_err(|error| error.into_api(fallback))?;

    // Opened before the status line is committed: a file that disappears after
    // the stat is a `400` with the route's fallback text, not a `200` that
    // breaks mid-body.
    let file = tokio::fs::File::open(&path).await.map_err(|_| ApiError::bad_request(fallback))?;

    let inline = query.inline.unwrap_or(false);
    let image_mime = INLINE_IMAGE_MIME
        .iter()
        .find(|(ext, _)| *ext == extension(&name))
        .map(|(_, mime)| *mime)
        .filter(|_| inline);
    let disposition = if image_mime.is_some() { "inline" } else { "attachment" };

    let response = Response::builder()
        .header(CONTENT_TYPE, image_mime.unwrap_or("application/octet-stream"))
        // RFC 5987 encoding, so a non-ASCII filename survives the header.
        .header(
            CONTENT_DISPOSITION,
            format!("{disposition}; filename*=UTF-8''{}", encode_uri_component(&name)),
        )
        // The declared type is the only type. Without this a browser is free to
        // sniff an `application/octet-stream` attachment as HTML and run it, which
        // turns "download this file" back into "execute this file on my origin".
        .header(X_CONTENT_TYPE_OPTIONS, "nosniff")
        // Inline means the browser renders it; the sandbox means rendering it is
        // all it can do. See `INLINE_CSP`.
        .header(CONTENT_SECURITY_POLICY, INLINE_CSP)
        // No `Content-Length`: the file can change between the stat and the read,
        // and a chunked body that ends early beats a truncated one that lies.
        .body(Body::from_stream(file_stream(file)))
        .map_err(|_| ApiError::bad_request(fallback))?;
    Ok(FileBytes(response))
}

/// Save an uploaded file into a directory under the home directory.
#[api(id = "fs_upload", group = "files", summary = "Upload a file")]
pub async fn fs_upload(
    State(state): State<AppState>,
    #[api(skip)] who: MaybeIdentity,
    ApiQuery(query): ApiQuery<FsUploadQuery>,
    multipart: Multipart,
) -> Result<Json<FsUpload>, ApiError> {
    let fallback = "cannot save file";
    let file = take_file_field(multipart).await.map_err(|error| error.into_api(fallback))?;
    save_upload(state.paths.as_ref(), Reach::of(&who), &query.dir, &file)
        .await
        .map(Json)
        .map_err(|error| error.into_api(fallback))
}

/// Store a file for attaching to a message.
#[api(id = "upload_attachment", group = "files", summary = "Upload an attachment")]
pub async fn upload_attachment(State(state): State<AppState>, multipart: Multipart) -> Result<Json<Attachment>, ApiError> {
    let fallback = "cannot save image";
    let file = take_file_field(multipart).await.map_err(|error| error.into_api(fallback))?;
    save_attachment(state.paths.as_ref(), &file)
        .await
        .map(Json)
        .map_err(|error| error.into_api(fallback))
}

/// Register this subsystem's routes.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router
        .get("/api/fs/list", fs_list)
        .get("/api/fs/read", fs_read)
        .get("/api/fs/download", fs_download)
        .merge(upload_route("/api/fs/upload", fs_upload, UPLOAD_LIMIT))
        .merge(upload_route("/api/uploads", upload_attachment, ATTACHMENT_LIMIT))
}

/// One multipart route carrying its own body limit.
///
/// A limit has to be layered onto the route rather than the application: the two
/// upload routes accept two different sizes, and every *other* route should keep
/// axum's 2 MiB default rather than inherit 200 MiB. `GotchaRouter` has no
/// per-route layer, so each route is built as a one-route router — which does
/// carry its own OpenAPI operation through [`gotcha::GotchaRouter::merge`] — and
/// layered before being merged back in.
fn upload_route<H, T>(path: &str, handler: H, limit: u64) -> ApiRouter
where
    H: gotcha::axum::handler::Handler<T, gotcha::GotchaContext<AppState, crate::config::AppConfig>>,
    T: 'static,
{
    ApiRouter::default()
        .post(path, handler)
        .layer(gotcha::axum::extract::DefaultBodyLimit::max((limit + MULTIPART_OVERHEAD) as usize))
}

/// The `multipart/form-data` body both upload routes read.
// gotcha derives nothing from a `Multipart` extractor — the generated
// `requestBody` names the media type and stops — so the one part the handlers
// read is documented by hand.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct FileUpload {
    /// The file part; the only one either route looks at.
    pub file: serde_json::Value,
}

/// Everything gotcha cannot derive about these operations.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe("GET", "/api/fs/list", 200, "List a directory under the home directory")
        .error("GET", "/api/fs/list", 400, "Invalid path")
        .describe(
            "GET",
            "/api/fs/read",
            200,
            "Read a text file under the home directory (capped at 256 KiB)",
        )
        .error("GET", "/api/fs/read", 400, "Invalid path")
        // Replaces the JSON `200` derived from `FileBytes`; see its doc comment.
        .binary("GET", "/api/fs/download", 200, "Raw file bytes as an attachment")
        .error("GET", "/api/fs/download", 400, "Invalid path")
        .describe("POST", "/api/fs/upload", 200, "File saved into the target directory (never overwrites)")
        .error("POST", "/api/fs/upload", 400, "Invalid upload")
        .describe("POST", "/api/uploads", 200, "File stored for attaching to a message")
        .error("POST", "/api/uploads", 400, "Invalid attachment")
        .request_body::<FileUpload>("POST", "/api/fs/upload", MULTIPART, "The file to save")
        .request_body::<FileUpload>("POST", "/api/uploads", MULTIPART, "The file to store");
}

/// The media type both upload routes read their body as.
const MULTIPART: &str = "multipart/form-data";

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> Paths {
        Paths::new(PathBuf::from("/home/tester"))
    }

    fn safe(requested: &str) -> Result<PathBuf, FsError> {
        resolve_safe(&paths(), Some(requested))
    }

    /// Whether `dir`'s filesystem folds case (APFS and HFS+ as macOS formats
    /// them, unlike ext4 on CI): a probe made there answers to its upper-case
    /// name as the very same file. The probe is gone again on return.
    fn case_insensitive(dir: &Path) -> bool {
        let probe = dir.join("case-probe");
        std::fs::write(&probe, b"").unwrap();
        let folded = file_id(&dir.join("CASE-PROBE")) == file_id(&probe);
        std::fs::remove_file(&probe).unwrap();
        folded
    }

    #[test]
    fn resolve_collapses_segments_textually() {
        assert_eq!(resolve(Path::new("/a/b/../c")), PathBuf::from("/a/c"));
        assert_eq!(resolve(Path::new("/a/./b//c/")), PathBuf::from("/a/b/c"));
        // `..` above the root is the root.
        assert_eq!(resolve(Path::new("/a/../../..")), PathBuf::from("/"));
    }

    #[test]
    fn resolve_makes_a_relative_path_absolute_against_the_working_directory() {
        let cwd = std::env::current_dir().expect("cwd");
        assert_eq!(resolve(Path::new("child")), cwd.join("child"));
    }

    #[test]
    fn a_tilde_expands_to_the_home_directory() {
        assert_eq!(safe("~").expect("~"), PathBuf::from("/home/tester"));
        assert_eq!(safe("  ~/docs  ").expect("~/docs"), PathBuf::from("/home/tester/docs"));
        assert_eq!(safe("").expect("empty"), PathBuf::from("/home/tester"));
    }

    #[test]
    fn a_doubled_separator_after_the_tilde_stays_inside_the_home_directory() {
        // `Path::join` would make this `/etc`; concatenated, as a shell reads it,
        // it is `/home/tester//etc`, which collapses back under the home.
        assert_eq!(safe("~//etc").expect("~//etc"), PathBuf::from("/home/tester/etc"));
    }

    #[test]
    fn traversal_and_outside_paths_are_refused() {
        for requested in ["/home/tester/../etc", "/etc/passwd", "~/../etc", "/home/tester2"] {
            match safe(requested) {
                Err(FsError::Access(message)) => assert_eq!(message, OUTSIDE_HOME, "{requested}"),
                other => panic!("{requested} was not refused: {other:?}"),
            }
        }
    }

    #[test]
    fn the_home_directory_itself_is_allowed() {
        assert_eq!(safe("/home/tester").expect("home"), PathBuf::from("/home/tester"));
    }

    #[test]
    fn upload_containment_excludes_the_directory_itself() {
        let paths = paths();
        let uploads = paths.uploads_dir();
        assert!(assert_under_uploads(&paths, &uploads.join("abc/x.png")).is_ok());
        assert!(assert_under_uploads(&paths, &uploads).is_err(), "the uploads directory is not itself an attachment");
        assert!(assert_under_uploads(&paths, Path::new("/etc/passwd")).is_err());
        assert!(assert_under_uploads(&paths, &uploads.join("../../../etc/passwd")).is_err());
    }

    #[test]
    fn a_name_keeps_only_its_last_segment() {
        assert_eq!(sanitize_name("/tmp/report.pdf"), "report.pdf");
        assert_eq!(sanitize_name("a/b/"), "b");
        assert_eq!(sanitize_name(r"C:\Users\me\x.txt"), "C:Usersmex.txt");
        assert_eq!(sanitize_name("/"), "");
        assert_eq!(sanitize_name(".."), "..");
        assert_eq!(sanitize_name(r"..\.."), "....");
    }

    #[test]
    fn an_extension_is_the_tail_after_the_last_dot() {
        assert_eq!(extension("photo.PNG"), "png");
        assert_eq!(extension(".png"), "png");
        // No dot at all: the whole name, not "".
        assert_eq!(extension("file"), "file");
    }

    /// The upload route's extraction: a name with no dot is its own extension.
    #[test]
    fn an_upload_whose_whole_name_is_an_extension_counts_as_an_image() {
        assert!(crate::sessions::util::is_image_ext(&extension("png")));
        assert!(crate::sessions::util::is_image_ext(&extension("JPG")));
        assert!(!crate::sessions::util::is_image_ext(&extension("notes")));
    }

    #[test]
    fn filenames_are_escaped_like_encode_uri_component() {
        assert_eq!(encode_uri_component("a b.txt"), "a%20b.txt");
        assert_eq!(encode_uri_component("héllo.png"), "h%C3%A9llo.png");
        assert_eq!(encode_uri_component("keep-_.!~*'()"), "keep-_.!~*'()");
        assert_eq!(encode_uri_component("a/b&c=d"), "a%2Fb%26c%3Dd");
    }

    /// Defence in depth: a device reaching this computer through Droppen
    /// lists, reads, downloads and uploads under the home directory —
    /// composer attachments included — but not
    /// into the server's own data or config directory, however the path gets
    /// there: directly, through `..`, through a symlink (a dangling one for
    /// an upload too), or in another case on a case-insensitive disk. A 403
    /// `not_from_cloud`. Local users are unaffected.
    #[tokio::test]
    async fn devices_through_droppen_are_fenced_out_of_the_servers_own_directories() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let (data, config) = (paths.data_dir(), paths.config_dir());
        let key = data.join("instances/default/agent/machine-key.json");
        std::fs::create_dir_all(key.parent().unwrap()).unwrap();
        std::fs::write(&key, b"{\"seed\":\"secret\"}").unwrap();
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("settings.json"), b"{}").unwrap();
        let attachment = paths.uploads_dir().join("u-1/shot.png");
        std::fs::create_dir_all(attachment.parent().unwrap()).unwrap();
        std::fs::write(&attachment, b"png").unwrap();
        std::fs::write(home.path().join("notes.txt"), b"hello").unwrap();
        std::os::unix::fs::symlink(&config, home.path().join("cfg")).unwrap();
        std::os::unix::fs::symlink(config.join("planted.sh"), home.path().join("dangling")).unwrap();
        let h = |rest: &str| format!("{}/{rest}", home.path().display());
        let private = |r: Result<(), FsError>| matches!(r, Err(FsError::Private));

        let cloud = Reach::Cloud;
        assert!(private(list_dir(&paths, cloud, Some(&data.to_string_lossy())).await.map(drop)));
        assert!(private(read_file_safe(&paths, cloud, &key.to_string_lossy()).await.map(drop)));
        assert!(private(stat_for_download(&paths, cloud, &key.to_string_lossy()).await.map(drop)));
        assert!(
            private(read_file_safe(&paths, cloud, &h("cfg/settings.json")).await.map(drop)),
            "through a symlink"
        );
        assert!(private(list_dir(&paths, cloud, Some(&h("cfg"))).await.map(drop)));
        assert!(
            private(
                read_file_safe(&paths, cloud, &h("x/../.config/remote-hand/settings.json"))
                    .await
                    .map(drop)
            ),
            "through .."
        );
        let config_case = h(".CONFIG/Remote-Hand/settings.json");
        let key_case = h(".LOCAL/Share/Remote-Hand/instances/default/agent/machine-key.json");
        if case_insensitive(home.path()) {
            // Another case names the very same directories: fenced.
            for variant in [&config_case, &key_case] {
                assert!(
                    private(read_file_safe(&paths, cloud, variant).await.map(drop)),
                    "another case on a case-insensitive disk: {variant}"
                );
            }
        } else {
            // Another case is another name: nothing is there...
            for variant in [&config_case, &key_case] {
                assert!(
                    matches!(read_file_safe(&paths, cloud, variant).await, Err(FsError::Other)),
                    "another case on a case-sensitive disk names nothing: {variant}"
                );
            }
            // ...and what is made there is not the config directory, which
            // the fence knows by identity, not by name: it reads.
            std::fs::create_dir_all(Path::new(&config_case).parent().unwrap()).unwrap();
            std::fs::write(&config_case, b"mine").unwrap();
            assert_eq!(read_file_safe(&paths, cloud, &config_case).await.unwrap().content, "mine");
            assert!(list_dir(&paths, cloud, Some(&h(".CONFIG/Remote-Hand"))).await.is_ok());
            assert!(private(list_dir(&paths, cloud, Some(&config.to_string_lossy())).await.map(drop)));
        }
        let file = UploadedFile {
            name: "planted.sh".into(),
            bytes: b"echo pwned".to_vec(),
        };
        assert!(private(save_upload(&paths, cloud, &config.to_string_lossy(), &file).await.map(drop)));
        let through = UploadedFile {
            name: "dangling".into(),
            bytes: b"echo pwned".to_vec(),
        };
        assert!(
            private(save_upload(&paths, cloud, &h(""), &through).await.map(drop)),
            "a dangling symlink in an allowed directory"
        );
        assert!(!config.join("planted.sh").exists(), "nothing landed");
        let refused = FsError::Private.into_api("x");
        assert_eq!(
            (refused.status().as_u16(), refused.body()["error"].as_str()),
            (403, Some("not_from_cloud"))
        );

        // What uses the computer goes on, attachments included.
        assert!(read_file_safe(&paths, cloud, &h("notes.txt")).await.is_ok());
        assert!(stat_for_download(&paths, cloud, &attachment.to_string_lossy()).await.is_ok());
        assert!(list_dir(&paths, cloud, Some("~")).await.is_ok());
        let note = UploadedFile {
            name: "new.txt".into(),
            bytes: b"x".to_vec(),
        };
        assert!(save_upload(&paths, cloud, "~", &note).await.is_ok());

        // Local users are not fenced.
        assert!(read_file_safe(&paths, Reach::Local, &key.to_string_lossy()).await.is_ok());
        assert!(list_dir(&paths, Reach::Local, Some(&h("cfg"))).await.is_ok());
    }

    /// The same fence at the routes: the request's identity decides.
    #[tokio::test]
    async fn the_file_routes_fence_bridged_devices_by_their_identity() {
        let home = tempfile::tempdir().expect("tempdir");
        let state = crate::api::test_support::state(home.path()).await;
        let key = state.paths.data_dir().join("instances/default/agent/machine-key.json");
        std::fs::create_dir_all(key.parent().unwrap()).unwrap();
        std::fs::write(&key, b"{}").unwrap();
        let who = |name: &str| {
            MaybeIdentity(Some(crate::auth::AuthIdentity {
                username: name.into(),
                token_sha256: None,
            }))
        };
        let path = key.to_string_lossy().into_owned();
        let refused = |e: ApiError| (e.status().as_u16(), e.body()["error"].as_str().map(str::to_string));
        let read = |name: &str| fs_read(State(state.clone()), who(name), ApiQuery(FsReadQuery { path: path.clone() }));
        assert_eq!(refused(read("cloud:d-1").await.unwrap_err()), (403, Some("not_from_cloud".into())));
        assert!(read("alice").await.is_ok());
        let list = fs_list(
            State(state.clone()),
            who("cloud:d-1"),
            ApiQuery(FsListQuery {
                path: Some(state.paths.data_dir().to_string_lossy().into_owned()),
            }),
        );
        assert_eq!(refused(list.await.unwrap_err()).0, 403);
        let download = fs_download(
            State(state.clone()),
            who("cloud:d-1"),
            ApiQuery(FsDownloadQuery {
                path: path.clone(),
                inline: None,
            }),
        );
        assert_eq!(refused(download.await.err().unwrap()).0, 403);

        // What the transcript links to in the managed Claude root, at the
        // routes too: a plan reads, a pasted image draws; the login does not.
        let claude = state.paths.claude_root(crate::claude_roots::ClaudeRoot::Managed);
        for (file, bytes) in [
            ("plans/p.md", &b"# plan"[..]),
            ("image-cache/s-1/1.png", b"png"),
            (".credentials.json", b"{}"),
        ] {
            std::fs::create_dir_all(claude.join(file).parent().unwrap()).unwrap();
            std::fs::write(claude.join(file), bytes).unwrap();
        }
        let at = |file: &str| claude.join(file).to_string_lossy().into_owned();
        let plan = fs_read(State(state.clone()), who("cloud:d-1"), ApiQuery(FsReadQuery { path: at("plans/p.md") }));
        assert_eq!(plan.await.expect("a plan reads").0.content, "# plan");
        let image = fs_download(
            State(state.clone()),
            who("cloud:d-1"),
            ApiQuery(FsDownloadQuery {
                path: at("image-cache/s-1/1.png"),
                inline: Some(true),
            }),
        );
        assert!(image.await.is_ok(), "a pasted image draws");
        let login = fs_read(
            State(state.clone()),
            who("cloud:d-1"),
            ApiQuery(FsReadQuery {
                path: at(".credentials.json"),
            }),
        );
        assert_eq!(refused(login.await.unwrap_err()), (403, Some("not_from_cloud".into())));
    }

    /// What the transcript links to in this instance's managed Claude root —
    /// a plan a session wrote and named, a pasted image it read back, the
    /// memory notes it saved, a tool's saved output — reads for the account's
    /// own devices (list, read, download), however the path gets there;
    /// nothing can be uploaded into it; and everything else in the root, the
    /// rest of the data directory and the config directory stay fenced: the
    /// login and its backups, what was pasted, the transcripts themselves, the
    /// machine key and the databases, another instance's root — through `..`,
    /// through a symlink in an opened directory, a symlink in an opened
    /// directory's place, a `memory` or `tool-results` at another depth, or
    /// another case on a case-insensitive disk.
    #[tokio::test]
    async fn devices_through_droppen_read_what_the_transcript_links_to_in_the_claude_root() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let claude = paths.claude_root(crate::claude_roots::ClaudeRoot::Managed);
        let data = paths.data_dir();
        let put = |path: &Path, bytes: &[u8]| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        };
        let session = claude.join("projects/-w/s-1");
        put(&claude.join("plans/p.md"), b"# plan");
        put(&claude.join("image-cache/s-1/1.png"), b"png");
        put(&session.join("tool-results/toolu_1.txt"), b"long output");
        put(&claude.join("projects/-w/memory/MEMORY.md"), b"- [deploy](deploy.md)");
        for secret in [
            ".credentials.json",
            ".claude.json",
            "backups/.claude.json.backup.1",
            "paste-cache/p.txt",
            "settings.json",
            "sessions/123.json",
            "projects/-w/s-1.jsonl",
            "projects/-w/tool-results/x.txt",
            "projects/-w/s-1/subagents/tool-results/x.txt",
            "tool-results/x.txt",
            "projects/memory/x.md",
            "projects/-w/s-1/memory/x.md",
        ] {
            put(&claude.join(secret), b"secret");
        }
        put(&paths.instance_dir().join("agent/machine-key.json"), b"{\"seed\":\"secret\"}");
        put(&paths.instance_db(), b"db");
        put(&paths.config_db(), b"db");
        put(&data.join("instances/other/claude/plans/q.md"), b"another instance's plan");
        put(&data.join("claude/plans/old.md"), b"the legacy root's plan");
        std::os::unix::fs::symlink("../.credentials.json", claude.join("plans/creds.md")).unwrap();
        std::os::unix::fs::symlink(paths.instance_dir().join("agent/machine-key.json"), session.join("tool-results/key.txt")).unwrap();
        std::os::unix::fs::symlink(claude.join("plans"), home.path().join("plans-link")).unwrap();
        let at = |rest: &str| claude.join(rest).to_string_lossy().into_owned();
        let cloud = Reach::Cloud;
        let private = |r: Result<(), FsError>| matches!(r, Err(FsError::Private));

        // Read again: list, read, download — also through a link in the home directory.
        assert_eq!(read_file_safe(&paths, cloud, &at("plans/p.md")).await.unwrap().content, "# plan");
        assert!(list_dir(&paths, cloud, Some(&at("plans"))).await.is_ok());
        assert!(stat_for_download(&paths, cloud, &at("image-cache/s-1/1.png")).await.is_ok());
        assert!(list_dir(&paths, cloud, Some(&at("image-cache/s-1"))).await.is_ok());
        assert!(read_file_safe(&paths, cloud, &at("projects/-w/s-1/tool-results/toolu_1.txt"))
            .await
            .is_ok());
        assert!(list_dir(&paths, cloud, Some(&at("projects/-w/s-1/tool-results"))).await.is_ok());
        assert!(stat_for_download(&paths, cloud, &at("projects/-w/memory/MEMORY.md")).await.is_ok());
        assert!(list_dir(&paths, cloud, Some(&at("projects/-w/memory"))).await.is_ok());
        let linked = format!("{}/plans-link/p.md", home.path().display());
        assert!(read_file_safe(&paths, cloud, &linked).await.is_ok(), "through a symlink");

        // Still fenced.
        for secret in [
            at(".credentials.json"),
            at(".claude.json"),
            at("backups/.claude.json.backup.1"),
            at("paste-cache/p.txt"),
            at("settings.json"),
            at("sessions/123.json"),
            at("projects/-w/s-1.jsonl"),
            at("projects/-w/tool-results/x.txt"),
            at("projects/-w/s-1/subagents/tool-results/x.txt"),
            at("tool-results/x.txt"),
            at("projects/memory/x.md"),
            at("projects/-w/s-1/memory/x.md"),
            at("plans/../.credentials.json"),
            at("plans/creds.md"),
            at("projects/-w/s-1/tool-results/key.txt"),
            paths.instance_dir().join("agent/machine-key.json").to_string_lossy().into_owned(),
            paths.instance_db().to_string_lossy().into_owned(),
            paths.config_db().to_string_lossy().into_owned(),
            data.join("instances/other/claude/plans/q.md").to_string_lossy().into_owned(),
            data.join("claude/plans/old.md").to_string_lossy().into_owned(),
        ] {
            assert!(private(read_file_safe(&paths, cloud, &secret).await.map(drop)), "{secret}");
            assert!(private(stat_for_download(&paths, cloud, &secret).await.map(drop)), "{secret}");
        }
        for dir in [
            at(""),
            at("projects"),
            at("projects/-w"),
            at("projects/-w/s-1"),
            at("backups"),
            at("paste-cache"),
        ] {
            assert!(private(list_dir(&paths, cloud, Some(&dir)).await.map(drop)), "{dir}");
        }

        // Another case opens nothing more. On a case-insensitive disk it
        // names the same directories, so an opened one stays open and the
        // rest stay fenced.
        let variants = [
            ("PLANS", "p.md"),
            ("Image-Cache/s-1", "1.png"),
            ("projects/-w/s-1/Tool-Results", "toolu_1.txt"),
            ("projects/-w/Memory", "MEMORY.md"),
        ];
        let secrets = [".Credentials.json", "Projects/-w/S-1.jsonl", "PROJECTS/-w/Tool-Results/x.txt"];
        if case_insensitive(home.path()) {
            for (dir, file) in variants {
                let variant = at(&format!("{dir}/{file}"));
                assert!(read_file_safe(&paths, cloud, &variant).await.is_ok(), "{variant}");
                assert!(list_dir(&paths, cloud, Some(&at(dir))).await.is_ok(), "{dir}");
            }
            for secret in secrets {
                assert!(private(read_file_safe(&paths, cloud, &at(secret)).await.map(drop)), "{secret}");
            }
        } else {
            // On a case-sensitive one another case is another name: nothing is
            // there, in the fenced root...
            for (dir, file) in variants {
                let variant = at(&format!("{dir}/{file}"));
                assert!(private(read_file_safe(&paths, cloud, &variant).await.map(drop)), "{variant}");
            }
            for secret in secrets {
                assert!(private(read_file_safe(&paths, cloud, &at(secret)).await.map(drop)), "{secret}");
            }
            // ...and a directory made under that name is not the opened one.
            for (dir, file) in variants {
                let variant = at(&format!("{dir}/{file}"));
                put(Path::new(&variant), b"made by hand");
                assert!(private(read_file_safe(&paths, cloud, &variant).await.map(drop)), "{variant}");
                assert!(private(list_dir(&paths, cloud, Some(&at(dir))).await.map(drop)), "{dir}");
            }
            assert_eq!(read_file_safe(&paths, cloud, &at("plans/p.md")).await.unwrap().content, "# plan");
        }

        // Read only: nothing is uploaded into them.
        let file = UploadedFile {
            name: "planted.md".into(),
            bytes: b"x".to_vec(),
        };
        for dir in [
            at("plans"),
            at("image-cache/s-1"),
            at("projects/-w/memory"),
            at("projects/-w/s-1/tool-results"),
        ] {
            assert!(private(save_upload(&paths, cloud, &dir, &file).await.map(drop)), "{dir}");
            assert!(!Path::new(&dir).join("planted.md").exists(), "{dir}");
        }
        let into = UploadedFile {
            name: "into-plans.md".into(),
            bytes: b"x".to_vec(),
        };
        std::os::unix::fs::symlink(claude.join("plans/into-plans.md"), home.path().join("into-plans.md")).unwrap();
        assert!(
            private(save_upload(&paths, cloud, "~", &into).await.map(drop)),
            "a dangling symlink into an opened directory"
        );
        assert!(!claude.join("plans/into-plans.md").exists());
        assert!(
            save_upload(&paths, Reach::Local, &at("plans"), &file).await.is_ok(),
            "local users are not fenced"
        );

        // A symlink in an opened directory's place opens nothing.
        std::fs::remove_dir_all(claude.join("plans")).unwrap();
        std::os::unix::fs::symlink(paths.instance_dir().join("agent"), claude.join("plans")).unwrap();
        assert!(private(read_file_safe(&paths, cloud, &at("plans/machine-key.json")).await.map(drop)));
    }

    /// The walk up from a path compares directories, and a hard link has no
    /// directory of its own: a second name for the login or the machine key,
    /// made in a reopened directory, would go through as whatever it is called
    /// there. So a device neither reads, downloads nor sees listed a
    /// hard-linked file in one (plans, pasted images, memory, saved tool
    /// output, attachments), however the path gets there, while the
    /// single-named files beside it read. Elsewhere in the home directory a
    /// hard-linked file (a build's output) reads; local users are not fenced.
    #[tokio::test]
    async fn devices_through_droppen_do_not_read_hard_links_in_reopened_directories() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let claude = paths.claude_root(crate::claude_roots::ClaudeRoot::Managed);
        let put = |path: &Path, bytes: &[u8]| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        };
        let credentials = claude.join(".credentials.json");
        let key = paths.instance_dir().join("agent/machine-key.json");
        let settings = paths.config_dir().join("settings.json");
        let login = claude.join(".claude.json");
        for secret in [&credentials, &key, &settings, &login] {
            put(secret, b"secret");
        }
        let cloud = Reach::Cloud;
        let private = |r: Result<(), FsError>| matches!(r, Err(FsError::Private));
        let names = |list: FsList| list.entries.into_iter().map(|e| e.name).collect::<Vec<_>>();

        for (dir, plain, linked, secret) in [
            (claude.join("plans"), "p.md", "creds.md", &credentials),
            (claude.join("image-cache/s-1"), "1.png", "2.png", &key),
            (claude.join("projects/-w/memory"), "MEMORY.md", "deploy.md", &settings),
            (claude.join("projects/-w/s-1/tool-results"), "toolu_1.txt", "toolu_2.txt", &login),
            (paths.uploads_dir().join("u-1"), "shot.png", "key.png", &key),
        ] {
            let (plain_name, linked_name) = (plain.to_string(), linked.to_string());
            let (plain, linked) = (dir.join(plain), dir.join(linked));
            put(&plain, b"mine");
            std::fs::hard_link(secret, &linked).unwrap();
            let (plain, linked) = (plain.to_string_lossy().into_owned(), linked.to_string_lossy().into_owned());
            let shown = names(list_dir(&paths, cloud, Some(&dir.to_string_lossy())).await.unwrap());

            assert_eq!(read_file_safe(&paths, cloud, &plain).await.unwrap().content, "mine", "{plain}");
            assert!(stat_for_download(&paths, cloud, &plain).await.is_ok(), "{plain}");
            assert!(private(read_file_safe(&paths, cloud, &linked).await.map(drop)), "{linked}");
            assert!(private(stat_for_download(&paths, cloud, &linked).await.map(drop)), "{linked}");
            assert!(shown.contains(&plain_name), "{plain} is listed");
            assert!(!shown.contains(&linked_name), "{linked} is not listed");

            // Through a symlink, which is judged where it lands.
            let via = home.path().join("via");
            let _ = std::fs::remove_file(&via);
            std::os::unix::fs::symlink(&linked, &via).unwrap();
            assert!(private(read_file_safe(&paths, cloud, &via.to_string_lossy()).await.map(drop)), "{linked}");

            // Local users are not fenced.
            assert_eq!(read_file_safe(&paths, Reach::Local, &linked).await.unwrap().content, "secret");
            let local = names(list_dir(&paths, Reach::Local, Some(&dir.to_string_lossy())).await.unwrap());
            assert!(local.contains(&linked_name), "{linked} is listed locally");
        }

        // Elsewhere in the home directory a hard-linked file still reads:
        // cargo links every build output to its hashed twin.
        let deps = home.path().join("app/target/debug/deps/app-0123abcd");
        put(&deps, b"\x7fELF");
        let built = home.path().join("app/target/debug/app");
        std::fs::hard_link(&deps, &built).unwrap();
        assert!(stat_for_download(&paths, cloud, &built.to_string_lossy()).await.is_ok());
        let shown = names(list_dir(&paths, cloud, Some(&built.parent().unwrap().to_string_lossy())).await.unwrap());
        assert!(shown.contains(&"app".to_string()));
    }

    #[test]
    fn a_requests_reach_is_cloud_only_for_bridged_devices() {
        let who = |name: &str| {
            MaybeIdentity(Some(crate::auth::AuthIdentity {
                username: name.into(),
                token_sha256: None,
            }))
        };
        assert_eq!(Reach::of(&who("cloud:d-abc")), Reach::Cloud);
        assert_eq!(Reach::of(&who("alice")), Reach::Local);
        assert_eq!(Reach::of(&MaybeIdentity(None)), Reach::Local);
    }

    /// Past the ceiling, the answer is the handler's own `400` and message.
    ///
    /// `vec![0u8; n]` of this size costs nothing to allocate (zeroed pages are
    /// mapped lazily) and both functions check the length before they touch the
    /// bytes, so nothing is ever faulted in.
    #[tokio::test]
    async fn an_over_limit_upload_keeps_the_typescript_refusal() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());

        let file = UploadedFile {
            name: "big.bin".to_string(),
            bytes: vec![0u8; (UPLOAD_LIMIT + 1) as usize],
        };
        match save_upload(&paths, Reach::Local, &home.path().to_string_lossy(), &file).await {
            Err(FsError::Access(message)) => assert_eq!(message, "file exceeds the 200 MiB upload limit"),
            other => panic!("an over-limit upload was not refused: {other:?}"),
        }

        let file = UploadedFile {
            name: "big.bin".to_string(),
            bytes: vec![0u8; (ATTACHMENT_LIMIT + 1) as usize],
        };
        match save_attachment(&paths, &file).await {
            Err(FsError::Access(message)) => assert_eq!(message, "file exceeds the 100 MiB upload limit"),
            other => panic!("an over-limit attachment was not refused: {other:?}"),
        }
    }
}
