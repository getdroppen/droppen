//! Installing and reading back a plugin package.
//!
//! A plugin package is a `pnpm pack` / `npm pack` tarball: a gzip'd tar whose
//! every entry sits under `package/`, whose `package/package.json` carries a
//! `droppen` manifest, and whose entry module is a relative `.js`/`.mjs` file
//! inside it (`docs/plugins/runtime.md` §1). An install unpacks and checks one
//! into `<instance>/plugin-packages/<id>/`; the directory is the installed
//! package, with its `package.json` at the top.
//!
//! # Refusing rather than repairing
//!
//! Every rule in §1 is a refusal here, and the message says which: an entry
//! that is not a regular file or a directory (symlinks and hard links above
//! all), an absolute path, a `..` segment, more than 2,000 files or 64 MiB
//! unpacked, a tarball over 32 MiB, a missing or `droppen`-less
//! `package.json`, a manifest the SDK refuses, an engine range this host does
//! not satisfy, and an entry outside the package or not a `.js`/`.mjs` file.
//! Nothing is unpacked with the tar's own modes or timestamps: files are
//! created afresh as regular files, so a mode bit or a link cannot survive
//! the trip.
//!
//! # Bounds a malicious tarball cannot argue with
//!
//! The tar crate reads GNU long-name and PAX header bodies whole, before this
//! module ever sees an entry, so the *decompressed stream itself* is held to
//! [`MAX_DECOMPRESSED_BYTES`] — the unpacked limit plus header headroom — by
//! a [`Read::take`] around the gzip decoder. A header that claims gigabytes
//! can therefore cost at most that much memory, and an entry whose path comes
//! out of one is refused before the path is ever copied into a message
//! ([`MAX_ENTRY_PATH`], and every echoed path is shortened). An entry that is
//! not a regular file may not carry a size at all: a directory header with a
//! 33 GiB size field is refused at the header, not skipped through.
//!
//! # The swap
//!
//! One version per plugin per instance. An install unpacks into a temporary
//! directory beside the final one and checks everything there. The swap
//! itself is one `renameatx_np(RENAME_SWAP)` (macOS) or
//! `renameat2(RENAME_EXCHANGE)` (Linux) where the filesystem takes it, so the
//! package directory names a complete package at every instant and a crash has
//! nothing to put back; elsewhere, and where the kernel refuses the exchange,
//! the old directory is moved aside first and a sweep restores it. A refusal
//! anywhere before the swap removes the temporary directory and leaves the
//! previous install exactly as it was.
//!
//! Installs and uninstalls take a per-instance `flock` on
//! `plugin-packages/.lock`, so two processes — the server and the CLI, or two
//! requests — serialize rather than race over the same id. The same lock
//! guards a sweep of `.tmp-*`, `.del-*` and `.old-*` leftovers: temporary and
//! deleted directories are removed, and an `.old-<id>.<uuid>` whose `<id>` is
//! missing is put back (the fallback swap's crash window), or removed when the
//! id is there again.
//!
//! # The install record
//!
//! The build id (`<version>-<first 12 hex of the tarball's sha256>`) names the
//! exact bytes that were installed, and no file inside the package records
//! it, so each install writes `.droppen-install.json` beside the package's
//! `package.json`. Reading an installed package back — for `GET /api/plugins`,
//! the feed, or the asset route — needs that record as well as
//! `package.json`; a directory without one is not an install, and is left
//! out with a warning rather than served. The file is the host's, not the
//! package's: a package that ships one (in any case; APFS would treat
//! `.DROPPEN-INSTALL.JSON` as the same name) is refused, and the asset route
//! never serves it.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use droppen_plugin_sdk::{sdk_version, valid_id, Manifest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The most one `POST /api/plugins` (or one `plugin install`) may carry.
pub const MAX_TARBALL_BYTES: usize = 32 * 1024 * 1024;
/// The most a package may unpack to, across its regular files.
pub const MAX_UNPACKED_BYTES: u64 = 64 * 1024 * 1024;
/// The most the *decompressed tar stream* may be, headers and padding
/// included. The unpacked limit plus headroom for 2,000 headers, their
/// padding and the long-name/PAX bodies tar reads before yielding an entry.
pub const MAX_DECOMPRESSED_BYTES: u64 = MAX_UNPACKED_BYTES + 16 * 1024 * 1024;
/// The most regular files a package may hold.
pub const MAX_FILES: usize = 2_000;
/// The longest tar entry path this host will even look at. A path over the
/// platform's `PATH_MAX` could not be created anyway, and a GNU long-name
/// header can otherwise hand this module megabytes of "path".
const MAX_ENTRY_PATH: usize = 4_096;
/// How much of an echoed path a refusal message carries. The path is the
/// attacker's, and the message goes back in a `400` body.
const SHOWN_PATH_BYTES: usize = 200;
/// The longest refusal message a package's own text may stretch to. Every
/// `Refused` message goes back in a `400` body, and a package can put a
/// megabyte-long name in its `package.json` as readily as in a tar header.
const MAX_MESSAGE_CHARS: usize = 512;
/// A bound on every entry, directories included. A 32 MiB tarball can hold at
/// most 65,536 headers; this only keeps a directory-heavy one from turning
/// into that many syscalls before the file count has a chance to refuse it.
const MAX_ENTRIES: usize = 10_000;
/// Where an install writes its own record, inside the package directory.
pub const RECORD_FILE: &str = ".droppen-install.json";
/// The record's format tag.
const RECORD_FORMAT: &str = "droppen.plugin-install.v1";
/// What every entry of a `pnpm pack` tarball starts with.
const PREFIX: &str = "package/";
/// The lock serializing installs and uninstalls of one instance's packages.
const LOCK_FILE: &str = ".lock";

/// Why an install or a removal was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    /// The package is not one this host may install; the message says why.
    Refused(String),
    /// The tarball (or the package it carries) is over a limit.
    TooLarge {
        /// Human-readable.
        message: String,
        /// The limit that was passed, in bytes.
        limit: u64,
    },
    /// The instance directory could not be read or written.
    Failed(String),
    /// There is no installed package of that id.
    NotFound(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(message) | Self::Failed(message) | Self::NotFound(message) => f.write_str(message),
            Self::TooLarge { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for InstallError {}

/// One plugin package as it sits unpacked on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The manifest its `package.json` declares.
    pub manifest: Manifest,
    /// `<version>-<first 12 hex of the tarball's sha256>`.
    pub build: String,
    /// The entry module, relative to the package directory and `/`-separated
    /// (`dist/index.js`).
    pub entry: String,
}

/// The record an install leaves in the package directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    format: String,
    id: String,
    version: String,
    build: String,
    sha256: String,
    entry: String,
    installed_at: String,
}

/// `<instance>/plugin-packages`.
pub fn packages_dir(instance_dir: &Path) -> PathBuf {
    instance_dir.join("plugin-packages")
}

/// `<instance>/plugin-packages/<id>`.
pub fn package_dir(instance_dir: &Path, id: &str) -> PathBuf {
    packages_dir(instance_dir).join(id)
}

/// Unpack, check and swap in `tarball` as the instance's copy of its plugin.
///
/// Returns what was installed, on the id the package names. The previous
/// install of that id, if any, is replaced; the plugin's documents
/// (`plugins/<id>/store.db`) are not touched.
pub fn install(instance_dir: &Path, tarball: &[u8]) -> Result<Installed, InstallError> {
    if tarball.len() > MAX_TARBALL_BYTES {
        return Err(too_large(
            format!("the tarball is over {MAX_TARBALL_BYTES} bytes"),
            MAX_TARBALL_BYTES as u64,
        ));
    }
    let sha256 = hex(&Sha256::digest(tarball));
    let parent = packages_dir(instance_dir);
    fs::create_dir_all(&parent).map_err(|error| failed(format!("cannot make {}: {error}", parent.display())))?;
    // Held for the whole install: the sweep, the unpack and the swap. The CLI
    // takes the same lock, so it serializes with a running server.
    let _lock = lock_packages(&parent)?;
    sweep_stale(&parent);

    let temp = parent.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
    let guard = TempDir::new(&temp);
    fs::create_dir(&temp).map_err(|error| failed(format!("cannot make {}: {error}", temp.display())))?;

    let (manifest, entry) = unpack(tarball, &temp)?;
    let id = manifest.id.as_str().to_string();
    let build = format!("{}-{}", manifest.version, &sha256[..12]);
    let record = Record {
        format: RECORD_FORMAT.to_string(),
        id: id.clone(),
        version: manifest.version.to_string(),
        build: build.clone(),
        sha256,
        entry: entry.clone(),
        installed_at: super::store::now(),
    };
    let encoded = serde_json::to_vec_pretty(&record).expect("an install record serialises");
    fs::write(temp.join(RECORD_FILE), encoded)
        .map_err(|error| failed(format!("cannot write the install record: {error}")))?;

    swap_into_place(&temp, &package_dir(instance_dir, &id), &id, &parent)?;
    guard.keep();

    Ok(Installed {
        manifest,
        build,
        entry,
    })
}

/// Remove an installed package. Its documents and its on/off decision stay.
///
/// The directory is renamed aside under the lock first, so the id stops naming
/// a package even if removing the bytes fails. The aside name is a `.del-`
/// one, which the sweep always deletes: an uninstall must never come back,
/// unlike the `.old-<id>.*` a fallback swap leaves for the sweep to restore.
pub fn uninstall(instance_dir: &Path, id: &str) -> Result<(), InstallError> {
    if !valid_id(id) {
        return Err(InstallError::NotFound(format!("no plugin {id:?} is installed")));
    }
    let parent = packages_dir(instance_dir);
    if !parent.exists() {
        return Err(InstallError::NotFound(format!("no plugin {id:?} is installed")));
    }
    let _lock = lock_packages(&parent)?;
    sweep_stale(&parent);
    let dir = package_dir(instance_dir, id);
    match fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() => {}
        _ => return Err(InstallError::NotFound(format!("no plugin {id:?} is installed"))),
    }
    let aside = parent.join(format!(".del-{}", uuid::Uuid::new_v4()));
    fs::rename(&dir, &aside).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            InstallError::NotFound(format!("no plugin {id:?} is installed"))
        } else {
            failed(format!("cannot move {} aside: {error}", dir.display()))
        }
    })?;
    if let Err(error) = fs::remove_dir_all(&aside) {
        tracing::warn!(path = %aside.display(), %error, "cannot remove an uninstalled plugin package; it is swept later");
    }
    Ok(())
}

/// The build an installed package records right now, if the record reads.
///
/// The asset route asks again after reading a file: a swap that lands between
/// its first look and its read must not let the new build's bytes go out under
/// the old (immutable) URL.
pub fn recorded_build(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join(RECORD_FILE)).ok()?;
    serde_json::from_str::<Record>(&text).ok().map(|record| record.build)
}

/// Read one installed package back. `Err` means the directory is not an
/// install — a record or a `package.json` that cannot be read, an id that
/// does not match its directory, an entry that has gone.
pub fn read_package(dir: &Path) -> Result<Installed, String> {
    let id = dir
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|id| valid_id(id))
        .ok_or_else(|| format!("{} is not a plugin directory", dir.display()))?;
    let text = fs::read_to_string(dir.join(RECORD_FILE)).map_err(|error| format!("{}: {error}", dir.join(RECORD_FILE).display()))?;
    let record: Record = serde_json::from_str(&text).map_err(|error| format!("{}: {error}", dir.join(RECORD_FILE).display()))?;
    if record.format != RECORD_FORMAT {
        return Err(format!("{} is in format {:?}, not {RECORD_FORMAT}", dir.join(RECORD_FILE).display(), record.format));
    }
    if record.id != id {
        return Err(format!("{} records plugin {:?}, not {id}", dir.join(RECORD_FILE).display(), record.id));
    }
    let package = serde_json::from_slice::<serde_json::Value>(&fs::read(dir.join("package.json")).map_err(|error| format!("{}: {error}", dir.join("package.json").display()))?)
        .map_err(|error| format!("{}: {error}", dir.join("package.json").display()))?;
    let manifest = package.manifest_of()?;
    if manifest.id.as_str() != id {
        return Err(format!("{} declares plugin {}, not {id}", dir.join("package.json").display(), manifest.id));
    }
    let version = manifest.version.to_string();
    let build_ok = record
        .build
        .strip_prefix(&format!("{version}-"))
        .is_some_and(|hex| hex.len() == 12 && hex.bytes().all(|b| b.is_ascii_hexdigit()));
    if !build_ok || record.version != version {
        return Err(format!(
            "{} records build {:?} for version {}, but package.json says {version}",
            dir.join(RECORD_FILE).display(),
            record.build,
            record.version
        ));
    }
    let entry = resolve_entry(dir, &record.entry)?;
    Ok(Installed {
        manifest,
        build: record.build,
        entry,
    })
}

/// Read, check and unpack the tarball into `into`, returning the manifest it
/// declares and its entry's relative path.
fn unpack(tarball: &[u8], into: &Path) -> Result<(Manifest, String), InstallError> {
    // The bound has to sit under the tar reader: tar-rs reads a long-name or
    // PAX header body whole before it yields the entry it describes.
    let decoder = flate2::read::GzDecoder::new(tarball);
    let mut stream = decoder.take(MAX_DECOMPRESSED_BYTES);
    let result = unpack_entries(&mut stream, into);
    // An error caused by the bound ("unexpected EOF during skip") is a package
    // that is too large, not a truncated one; the limit is checked first for
    // that reason.
    if stream.limit() == 0 {
        return Err(too_large(
            format!("the package decompresses to more than {MAX_DECOMPRESSED_BYTES} bytes"),
            MAX_DECOMPRESSED_BYTES,
        ));
    }
    result
}

/// [`unpack`]'s body, with the bounded stream already made. Everything here
/// returns normally — dropping the tar reader with it — so the caller can tell
/// a bound hit from a malformed archive.
fn unpack_entries(stream: &mut impl Read, into: &Path) -> Result<(Manifest, String), InstallError> {
    let mut files = 0usize;
    let mut entries = 0usize;
    let mut unpacked = 0u64;
    let mut seen = std::collections::HashSet::new();
    {
        let mut archive = tar::Archive::new(stream);
        for entry in archive.entries().map_err(|error| refused(format!("not a gzip'd tar: {error}")))? {
            let mut entry = entry.map_err(|error| refused(format!("not a gzip'd tar: {error}")))?;
            entries += 1;
            if entries > MAX_ENTRIES {
                return Err(refused(format!("more than {MAX_ENTRIES} entries")));
            }
            let raw = entry.path_bytes();
            if raw.len() > MAX_ENTRY_PATH {
                return Err(refused(format!("an entry's path is over {MAX_ENTRY_PATH} bytes")));
            }
            // The whole path is used to name the file; only `shown` — the same
            // path, shortened — goes into messages.
            let name = String::from_utf8_lossy(&raw).into_owned();
            let shown = describe_path(&raw);
            let kind = entry.header().entry_type();
            if !(kind.is_file() || kind.is_dir()) {
                return Err(refused(format!("{shown} is not a regular file or a directory")));
            }
            let size = entry.size();
            if !kind.is_file() && size != 0 {
                // A directory or a link header with a size would have tar skip
                // that many bytes: the 33 GiB directory trick. A real one is 0.
                return Err(refused(format!("{shown} carries data but is not a regular file")));
            }
            let path = package_path(&name, &shown)?;
            if path.is_empty() {
                continue;
            }
            if is_record_name(&path) {
                return Err(refused(format!("{shown} is reserved for the install's own record")));
            }
            if !seen.insert(path.clone()) {
                return Err(refused(format!("{shown} appears twice")));
            }
            if kind.is_file() {
                files += 1;
                if files > MAX_FILES {
                    return Err(refused(format!("more than {MAX_FILES} files")));
                }
                unpacked = unpacked.saturating_add(size);
                if unpacked > MAX_UNPACKED_BYTES {
                    return Err(too_large(
                        format!("the package unpacks to more than {MAX_UNPACKED_BYTES} bytes"),
                        MAX_UNPACKED_BYTES,
                    ));
                }
                write_file(into, &path, &mut entry, size).map_err(|error| refused(format!("{shown}: {error}")))?;
            } else {
                fs::create_dir_all(into.join(Path::new(&path))).map_err(|error| refused(format!("{shown}: {error}")))?;
            }
        }
    }

    let package_file = into.join("package.json");
    let bytes = fs::read(&package_file).map_err(|_| refused("the package has no package/package.json".to_string()))?;
    let package: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| refused(format!("package/package.json: {error}")))?;
    let manifest = package.manifest_of().map_err(|message| refused(message))?;
    if !manifest.compatible_with(&sdk_version()) {
        return Err(refused(format!(
            "it needs Droppen plugin SDK {}; this host implements {}",
            manifest.engines.droppen,
            sdk_version()
        )));
    }
    let entry = package.entry_of(into).map_err(|message| refused(message))?;
    Ok((manifest, entry))
}

/// An entry path as a refusal may echo it: at most [`SHOWN_PATH_BYTES`], with
/// an ellipsis when it was longer. The path is the attacker's and the message
/// travels back in a `400` body.
fn describe_path(raw: &[u8]) -> String {
    let cut = raw.len().min(SHOWN_PATH_BYTES);
    let mut text = String::from_utf8_lossy(&raw[..cut]).into_owned();
    if raw.len() > cut {
        text.push('…');
    }
    text
}

/// Whether `path` is the install record's own name at the package root.
/// Case-insensitive because APFS and NTFS are: a package could otherwise ship
/// `.DROPPEN-INSTALL.JSON` and collide with the record this host writes.
fn is_record_name(path: &str) -> bool {
    !path.contains('/') && is_record_path(path)
}

/// Whether any segment of `path` is the install record's name, in any case.
/// The asset route refuses these: the record is the host's bookkeeping, not
/// the package's code.
pub fn is_record_path(path: &str) -> bool {
    path.split('/').any(|segment| segment.eq_ignore_ascii_case(RECORD_FILE))
}

/// One entry's path under the package directory, `/`-separated, or `""` for
/// the `package/` directory itself. `name` is the whole path; `shown` is the
/// same path shortened for the refusal messages.
fn package_path(name: &str, shown: &str) -> Result<String, InstallError> {
    // `/` is the only separator an npm tarball uses; a backslash would be one
    // on Windows, where the same name would mean a different path.
    if name.contains('\\') || name.contains('\0') {
        return Err(refused(format!("{shown:?} is not a path a package may carry")));
    }
    if name.starts_with('/') {
        return Err(refused(format!("{shown:?} is an absolute path")));
    }
    let stripped = name.strip_prefix(PREFIX).ok_or_else(|| refused(format!("{shown:?} is not under package/")))?;
    if stripped.is_empty() {
        return Ok(String::new());
    }
    let mut parts = Vec::new();
    for part in stripped.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(refused(format!("{shown:?} steps out of the package"))),
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(refused(format!("{shown:?} does not name a file inside the package")));
    }
    Ok(parts.join("/"))
}

/// Write one regular file, never following or replacing anything already there.
fn write_file(into: &Path, path: &str, data: &mut impl Read, size: u64) -> io::Result<()> {
    let target = into.join(Path::new(path));
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&target)?;
    let copied = io::copy(&mut data.take(size), &mut file)?;
    if copied != size {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the entry ended before its size"));
    }
    file.flush()?;
    Ok(())
}

/// Reading a package's `package.json`.
trait PackageJson {
    /// The manifest it declares: its `droppen` object, with the package's
    /// top-level `version` and, when the object gives no name, the id as one.
    fn manifest_of(&self) -> Result<Manifest, String>;

    /// The entry a package names, before it is checked: `exports`, `module`
    /// and `main`, nullish fallbacks and all. A name that is there but not a
    /// string is a refusal rather than a reason to fall through to `index.js`.
    fn entry_field(&self) -> Result<String, String>;

    /// The entry module's relative path: `exports["."]` (a string, or its
    /// `import`/`default`), else `module`, else `main`, else `index.js`; a
    /// relative `.js`/`.mjs` file that exists.
    fn entry_of(&self, dir: &Path) -> Result<String, String>;
}

impl PackageJson for serde_json::Value {
    fn manifest_of(&self) -> Result<Manifest, String> {
    let object = self.as_object().ok_or("package/package.json is not a JSON object")?;
    let droppen = object.get("droppen").ok_or("package/package.json has no \"droppen\" manifest")?;
    let mut fields = droppen
        .as_object()
        .cloned()
        .ok_or("package/package.json's \"droppen\" is not an object")?;
    let id = fields
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or("the droppen manifest has no id")?
        .to_string();
    if !valid_id(&id) {
        return Err(format!(
            "{id:?} is not a plugin id: 2-32 characters of a-z, 0-9 or '-', starting with a letter"
        ));
    }
    let version = object.get("version").ok_or("package/package.json has no version")?;
    fields.insert("version".to_string(), version.clone());
    if fields.get("name").is_none_or(serde_json::Value::is_null) {
        fields.insert("name".to_string(), serde_json::Value::String(id));
    }
    let manifest: Manifest = serde_json::from_value(serde_json::Value::Object(fields)).map_err(|error| format!("the droppen manifest is not one: {error}"))?;
    let problems = manifest.problems();
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    Ok(manifest)
    }

    fn entry_of(&self, dir: &Path) -> Result<String, String> {
        let rel = self.entry_field()?;
        resolve_entry(dir, &rel)
    }

    fn entry_field(&self) -> Result<String, String> {
    let rel = match self.get("exports") {
        Some(serde_json::Value::String(rel)) => Some(serde_json::Value::String(rel.clone())),
        Some(exports @ serde_json::Value::Object(map)) => {
            // `exp["."] ?? exp`: an absent (or null) dot key means the object
            // itself is the dot entry.
            let dot = map.get(".").filter(|value| !value.is_null()).unwrap_or(exports);
            match dot {
                serde_json::Value::String(rel) => Some(serde_json::Value::String(rel.clone())),
                serde_json::Value::Object(dot) => dot
                    .get("import")
                    .filter(|value| !value.is_null())
                    .or_else(|| dot.get("default"))
                    .cloned(),
                _ => None,
            }
        }
        _ => None,
    };
    // `rel ??= module ?? main ?? "index.js"`.
    let rel = rel.filter(|value| !value.is_null()).unwrap_or_else(|| {
        serde_json::Value::String(
            ["module", "main"]
                .iter()
                .find_map(|key| self.get(*key).and_then(serde_json::Value::as_str))
                .unwrap_or("index.js")
                .to_string(),
        )
    });
    match rel {
        serde_json::Value::String(rel) => Ok(rel),
        _ => Err("cannot tell the entry module from \"exports\"".to_string()),
    }
    }
}

/// Check that `rel` is a relative `.js`/`.mjs` file inside `dir`, and return it
/// normalised (`./dist/index.js` → `dist/index.js`).
fn resolve_entry(dir: &Path, rel: &str) -> Result<String, String> {
    if rel.contains('\\') || rel.contains('\0') {
        return Err(format!("the entry {rel:?} is not a path a package may carry"));
    }
    if rel.starts_with('/') || Path::new(rel).is_absolute() {
        return Err(format!("the entry {rel:?} is absolute; it must be inside the package"));
    }
    let mut parts = Vec::new();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(format!("the entry {rel:?} steps out of the package")),
            part => parts.push(part),
        }
    }
    let entry = parts.join("/");
    if entry.is_empty() {
        return Err(format!("the entry {rel:?} does not name a file"));
    }
    if !(entry.ends_with(".js") || entry.ends_with(".mjs")) {
        return Err(format!("the entry {entry:?} is not a .js or .mjs file"));
    }
    let target = dir.join(Path::new(&entry));
    let meta = fs::symlink_metadata(&target).map_err(|_| format!("the entry {entry:?} does not exist — is the plugin built?"))?;
    if !meta.file_type().is_file() {
        return Err(format!("the entry {entry:?} is not a regular file"));
    }
    // A path made only of ordinary components cannot leave `dir`, and the
    // extraction above never creates a symlink; this is the check that keeps
    // the two claims together should either ever change.
    debug_assert!(target.components().all(|part| !matches!(part, Component::ParentDir)));
    Ok(entry)
}

/// Move `temp` to `final_dir`, replacing what is there in one step where the
/// filesystem can, and by a move-aside + sweep elsewhere.
fn swap_into_place(temp: &Path, final_dir: &Path, id: &str, parent: &Path) -> Result<(), InstallError> {
    if fs::symlink_metadata(final_dir).is_err() {
        return fs::rename(temp, final_dir)
            .map_err(|error| failed(format!("cannot move the new package into {}: {error}", final_dir.display())));
    }
    match exchange(temp, final_dir) {
        Ok(()) => {
            // `temp` now holds the previous package; removing it is cleanup,
            // not part of the swap.
            if let Err(error) = fs::remove_dir_all(temp) {
                tracing::warn!(path = %temp.display(), %error, "cannot remove the replaced plugin package; it is swept later");
            }
            Ok(())
        }
        Err(error) => {
            tracing::warn!(path = %final_dir.display(), %error, "the filesystem will not exchange directories; moving the old one aside instead");
            // `.old-<id>.<uuid>`: the id is what the sweep needs to put it
            // back if a crash lands between the two renames.
            let old = parent.join(format!(".old-{id}.{}", uuid::Uuid::new_v4()));
            fs::rename(final_dir, &old)
                .map_err(|error| failed(format!("cannot move the old package aside: {error}")))?;
            match fs::rename(temp, final_dir) {
                Ok(()) => {
                    if let Err(error) = fs::remove_dir_all(&old) {
                        tracing::warn!(path = %old.display(), %error, "cannot remove the replaced plugin package; it is swept later");
                    }
                    Ok(())
                }
                Err(error) => {
                    if let Err(restore) = fs::rename(&old, final_dir) {
                        tracing::error!(path = %final_dir.display(), %restore, "cannot put the previous plugin package back; the sweep will");
                    }
                    Err(failed(format!("cannot move the new package into {}: {error}", final_dir.display())))
                }
            }
        }
    }
}

/// Exchange two directory names in one operation, or fail if this platform or
/// filesystem cannot. Linux and macOS both can; the fallback in
/// [`swap_into_place`] covers the rest.
#[cfg(target_os = "linux")]
fn exchange(left: &Path, right: &Path) -> io::Result<()> {
    let left = c_path(left)?;
    let right = c_path(right)?;
    // SAFETY: both pointers are NUL-terminated strings owned for the call.
    let result = unsafe { libc::renameat2(libc::AT_FDCWD, left.as_ptr(), libc::AT_FDCWD, right.as_ptr(), libc::RENAME_EXCHANGE) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
fn exchange(left: &Path, right: &Path) -> io::Result<()> {
    let left = c_path(left)?;
    let right = c_path(right)?;
    // SAFETY: both pointers are NUL-terminated strings owned for the call.
    let result = unsafe { libc::renameatx_np(libc::AT_FDCWD, left.as_ptr(), libc::AT_FDCWD, right.as_ptr(), libc::RENAME_SWAP) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn exchange(_left: &Path, _right: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "no atomic directory exchange on this platform"))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn c_path(path: &Path) -> io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a path with a NUL"))
}

/// One instance's packages are installed and removed one at a time: a `flock`
/// on `plugin-packages/.lock`, which the CLI takes too, so a running server
/// and `plugin install` cannot both be unpacking into the same id.
struct PackagesLock {
    #[allow(dead_code)]
    file: fs::File,
}

fn lock_packages(parent: &Path) -> Result<PackagesLock, InstallError> {
    use std::os::unix::io::AsRawFd;
    let path = parent.join(LOCK_FILE);
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| failed(format!("cannot open {}: {error}", path.display())))?;
    loop {
        // SAFETY: `file` is an open descriptor this process owns for the call,
        // and LOCK_EX takes no other arguments.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(PackagesLock { file });
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(failed(format!("cannot lock {}: {error}", path.display())));
        }
    }
}

/// Clean up what a crash or an interrupted install left beside the packages:
/// temporary and `.del-` directories go, and an `.old-<id>.<uuid>` whose id is
/// missing is put back (a fallback swap that died between its two renames).
///
/// A `.del-` is an uninstall's renamed-aside copy and is never restored, even
/// if removing it failed halfway: an uninstall that came back would be worse
/// than a few bytes left on disk.
///
/// Called with the packages lock held, so nothing is moving underneath it.
fn sweep_stale(parent: &Path) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if name.starts_with(".tmp-") || name.starts_with(".del-") {
            remove_tree(&path);
            continue;
        }
        let Some(rest) = name.strip_prefix(".old-") else {
            continue;
        };
        if let Some((id, _uuid)) = rest.split_once('.') {
            if valid_id(id) {
                let target = parent.join(id);
                if !target.exists() && fs::rename(&path, &target).is_ok() {
                    tracing::info!(plugin = id, "put back a plugin package an interrupted install had moved aside");
                    continue;
                }
            }
        }
        remove_tree(&path);
    }
}

fn remove_tree(path: &Path) {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            let _ = fs::remove_dir_all(path);
        }
        Ok(_) => {
            let _ = fs::remove_file(path);
        }
        Err(_) => {}
    }
}

/// Removes a temporary directory unless the install took it over.
struct TempDir {
    path: PathBuf,
    keep: bool,
}

impl TempDir {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            keep: false,
        }
    }

    fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn refused(message: impl Into<String>) -> InstallError {
    let mut message = message.into();
    if message.chars().count() > MAX_MESSAGE_CHARS {
        let cut = message.char_indices().nth(MAX_MESSAGE_CHARS).map_or(message.len(), |(index, _)| index);
        message.truncate(cut);
        message.push('…');
    }
    InstallError::Refused(message)
}

fn failed(message: impl Into<String>) -> InstallError {
    InstallError::Failed(message.into())
}

fn too_large(message: impl Into<String>, limit: u64) -> InstallError {
    InstallError::TooLarge {
        message: message.into(),
        limit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::testing::{package, tarball, Entry};

    fn temp() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let instance = dir.path().join("instances").join("t");
        (dir, instance)
    }

    /// Install `bytes` into a fresh instance, expecting a refusal, and check
    /// that no package (nor a temporary one) was left behind.
    fn refusal(bytes: &[u8]) -> String {
        let (_dir, instance) = temp();
        let message = match install(&instance, bytes) {
            Err(InstallError::Refused(message)) => message,
            Err(InstallError::TooLarge { message, .. }) => message,
            Err(other) => panic!("expected a refusal, got {other:?}"),
            Ok(installed) => panic!("expected a refusal, installed {}", installed.manifest.id),
        };
        let leftovers = leftover_work_dirs(&instance);
        assert!(leftovers.is_empty(), "{message:?} left something behind: {leftovers:?}");
        message
    }

    #[test]
    fn a_minimal_package_installs_with_its_build_and_entry() {
        let (_dir, instance) = temp();
        let installed = install(&instance, &tarball(&package("tasks", "0.1.0"), &[("dist/index.js", b"export default {};\n")])).unwrap();
        assert_eq!(installed.manifest.id.as_str(), "tasks");
        assert_eq!(installed.entry, "dist/index.js");
        assert!(installed.build.starts_with("0.1.0-") && installed.build.len() == "0.1.0-".len() + 12);
        let dir = package_dir(&instance, "tasks");
        assert!(dir.join("package.json").is_file(), "the package's package.json at the top");
        assert!(dir.join("dist/index.js").is_file());
        let read = read_package(&dir).unwrap();
        assert_eq!(read, installed, "what was installed is what reads back");
        assert_eq!(read.manifest.name.localize("en"), "tasks", "a package without a name takes its id");
    }

    #[test]
    fn a_reinstall_replaces_the_package_and_a_refusal_keeps_the_previous_one() {
        let (_dir, instance) = temp();
        install(&instance, &tarball(&package("tasks", "0.1.0"), &[("dist/index.js", b"one")])).unwrap();
        let first = read_package(&package_dir(&instance, "tasks")).unwrap();
        install(&instance, &tarball(&package("tasks", "0.2.0"), &[("dist/index.js", b"two")])).unwrap();
        let second = read_package(&package_dir(&instance, "tasks")).unwrap();
        assert_eq!(second.manifest.version.to_string(), "0.2.0");
        assert_ne!(second.build, first.build);
        assert_eq!(fs::read(package_dir(&instance, "tasks").join("dist/index.js")).unwrap(), b"two");
        assert!(!package_dir(&instance, "tasks").join("dist/old.js").exists());

        let mut broken = package("tasks", "0.3.0");
        broken["droppen"]["id"] = serde_json::json!("Tasks");
        assert!(install(&instance, &tarball(&broken, &[("dist/index.js", b"three")])).is_err());
        let after = read_package(&package_dir(&instance, "tasks")).unwrap();
        assert_eq!(after, second, "the refused install left 0.2.0 in place");
        let leftovers = leftover_work_dirs(&instance);
        assert!(leftovers.is_empty(), "no temporary directory outlived the install: {leftovers:?}");
    }

    /// `.tmp-*`, `.del-*` and `.old-*` entries left beside the packages (the
    /// `.lock` is expected and is not one).
    fn leftover_work_dirs(instance: &Path) -> Vec<String> {
        read_dir_names(&packages_dir(instance))
            .into_iter()
            .filter(|name| name.starts_with(".tmp-") || name.starts_with(".del-") || name.starts_with(".old-"))
            .collect()
    }

    #[test]
    fn uninstalling_removes_the_package_and_nothing_else() {
        let (_dir, instance) = temp();
        install(&instance, &tarball(&package("tasks", "0.1.0"), &[("dist/index.js", b"x")])).unwrap();
        uninstall(&instance, "tasks").unwrap();
        assert!(!package_dir(&instance, "tasks").exists());
        assert!(leftover_work_dirs(&instance).is_empty(), "the renamed-aside copy was removed too");
        assert!(matches!(uninstall(&instance, "tasks"), Err(InstallError::NotFound(_))));
        assert!(matches!(uninstall(&instance, "../x"), Err(InstallError::NotFound(_))));
    }

    #[test]
    fn a_package_may_not_ship_the_install_record_in_any_case() {
        for name in [".droppen-install.json", ".DROPPEN-INSTALL.JSON"] {
            let message = refusal(&crate::plugins::testing::raw_tarball(&[
                Entry::file("package/package.json", package("tasks", "0.1.0").to_string().into_bytes()),
                Entry::file(format!("package/{name}"), b"{}"),
            ]));
            assert!(message.contains("reserved for the install's own record"), "{name}: {message}");
        }
    }

    /// tar-rs reads a GNU long-name header body whole before yielding an
    /// entry, so a header that claims a gigabyte is the bomb the decompressed
    /// bound exists for. The refusal must be short (the body is not echoed)
    /// and must not take long.
    #[test]
    fn a_gnu_long_name_header_that_claims_a_gigabyte_is_refused_cheaply() {
        let started = std::time::Instant::now();
        let message = refusal(&crate::plugins::testing::raw_tarball(&[Entry::long_name(
            vec![b'a'; 2 * 1024 * 1024],
            1024 * 1024 * 1024,
        )]));
        assert!(message.len() < 1024, "the refusal echoed the header body: {} bytes", message.len());
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "took {:?}", started.elapsed());
    }

    /// The same header carrying a real (not lying) name: tar hands this module
    /// the whole path, and it must be refused by length rather than copied.
    #[test]
    fn a_path_from_a_long_name_header_is_refused_by_length() {
        let mut name = b"package/".to_vec();
        name.extend(std::iter::repeat(b'a').take(MAX_ENTRY_PATH + 1));
        name.extend_from_slice(b".js");
        let declared = name.len() as u64;
        let message = refusal(&crate::plugins::testing::raw_tarball(&[
            Entry::long_name(name, declared),
            Entry::file("package/short.js", b"x"),
        ]));
        assert!(message.contains("path is over"), "{message}");
        assert!(message.len() < 1024, "the refusal echoed the path: {} bytes", message.len());
    }

    /// A path over the 200-byte message cap but under the refusal limit is
    /// still the path the file is extracted to: only messages are shortened.
    #[test]
    fn a_long_but_legal_path_is_used_whole() {
        let (_dir, instance) = temp();
        // Over the 200-byte message cap, with every component under NAME_MAX.
        let deep = format!("dist/{}/{}.js", "a".repeat(120), "b".repeat(120));
        let mut pkg = package("tasks", "0.1.0");
        pkg["exports"] = serde_json::json!({ ".": format!("./{deep}") });
        let long = format!("package/{deep}");
        let entries = vec![
            Entry::long_name(long.clone().into_bytes(), long.len() as u64),
            Entry::file("package/short.js", b"export default {};\n".to_vec()),
            Entry::file("package/package.json", serde_json::to_vec(&pkg).unwrap()),
        ];
        let installed = install(&instance, &crate::plugins::testing::raw_tarball(&entries)).unwrap();
        assert_eq!(installed.entry, deep);
        assert!(package_dir(&instance, "tasks").join(&deep).is_file(), "the whole name was written");
    }

    /// The same cap holds for package.json's own strings: a header is not the
    /// only place a megabyte-long name can hide.
    #[test]
    fn a_huge_name_in_package_json_is_not_echoed() {
        let mut pkg = package("tasks", "0.1.0");
        pkg.as_object_mut().expect("an object").remove("exports");
        pkg["main"] = serde_json::json!(format!("{}.js", "a".repeat(1024 * 1024)));
        let message = refusal(&tarball(&pkg, &[("dist/index.js", b"x")]));
        assert!(message.len() < 1024, "the refusal echoed the entry: {} bytes", message.len());
    }

    /// Hitting the decompression bound mid-entry is "too large", not "not a
    /// gzip'd tar": the bound is checked before the tar error is reported.
    #[test]
    fn exceeding_the_decompression_bound_is_too_large() {
        let message = refusal(&crate::plugins::testing::streamed_long_name(
            100 * 1024 * 1024,
            MAX_DECOMPRESSED_BYTES + 1024 * 1024,
        ));
        assert!(message.contains("decompresses to more than"), "{message}");
    }

    /// A directory header with a 33 GiB size would have tar skip 33 GiB of
    /// stream. The header is refused before any of it is read.
    #[test]
    fn a_directory_header_with_a_huge_size_is_refused_at_the_header() {
        let started = std::time::Instant::now();
        let message = refusal(&crate::plugins::testing::raw_tarball(&[Entry::dir_with_size(
            "package/dist",
            33 * 1024 * 1024 * 1024,
        )]));
        assert!(message.contains("carries data but is not a regular file"), "{message}");
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "took {:?}", started.elapsed());
    }

    /// Two installs of one id at once serialize on `plugin-packages/.lock`:
    /// both succeed, one package is left, and no work directory survives.
    #[test]
    fn installs_of_one_id_from_two_threads_leave_one_whole_package() {
        let (_dir, instance) = temp();
        let one = tarball(&package("tasks", "0.1.0"), &[("dist/index.js", b"one")]);
        let two = tarball(&package("tasks", "0.2.0"), &[("dist/index.js", b"two")]);
        for _ in 0..50 {
            let (first_instance, second_instance) = (instance.clone(), instance.clone());
            let (first_bytes, second_bytes) = (one.clone(), two.clone());
            let first = std::thread::spawn(move || install(&first_instance, &first_bytes));
            let second = std::thread::spawn(move || install(&second_instance, &second_bytes));
            let first = first.join().unwrap();
            let second = second.join().unwrap();
            assert!(first.is_ok(), "{first:?}");
            assert!(second.is_ok(), "{second:?}");
            let read = read_package(&package_dir(&instance, "tasks")).expect("one whole package");
            assert!(matches!(read.manifest.version.to_string().as_str(), "0.1.0" | "0.2.0"), "{}", read.manifest.version);
        }
        assert!(leftover_work_dirs(&instance).is_empty());
    }

    /// The one-step swap the install relies on where the platform has it; a
    /// filesystem that refuses falls back, but this host's does not.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn the_platform_can_exchange_two_directories_in_one_step() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("left");
        let right = dir.path().join("right");
        fs::create_dir_all(left.join("was-left")).unwrap();
        fs::create_dir_all(right.join("was-right")).unwrap();
        exchange(&left, &right).expect("the platform exchanges directories");
        assert!(left.join("was-right").is_dir(), "the contents swapped");
        assert!(right.join("was-left").is_dir());
    }

    /// An uninstall's renamed-aside copy is swept, never put back: a
    /// half-removed uninstall must not come back on the next install.
    #[test]
    fn an_uninstalls_aside_copy_is_swept_not_restored() {
        let (_dir, instance) = temp();
        install(&instance, &tarball(&package("tasks", "0.1.0"), &[("dist/index.js", b"x")])).unwrap();
        let parent = packages_dir(&instance);
        // What `uninstall` leaves if `remove_dir_all` fails halfway.
        let aside = parent.join(format!(".del-{}", uuid::Uuid::new_v4()));
        fs::rename(package_dir(&instance, "tasks"), &aside).unwrap();
        // Any later sweep — here the next uninstall, which then finds nothing.
        assert!(matches!(uninstall(&instance, "tasks"), Err(InstallError::NotFound(_))));
        assert!(!package_dir(&instance, "tasks").exists(), "the plugin stays uninstalled");
        assert!(!aside.exists(), "the aside copy is deleted, not restored");
    }

    /// An interrupted fallback swap leaves `.old-<id>.<uuid>`; the next
    /// install puts it back, and stale temporaries go.
    #[test]
    fn a_sweep_restores_an_orphaned_old_copy_and_removes_stale_temporaries() {
        let (_dir, instance) = temp();
        install(&instance, &tarball(&package("tasks", "0.1.0"), &[("dist/index.js", b"one")])).unwrap();
        let parent = packages_dir(&instance);
        let orphan = parent.join(".old-tasks.00000000-0000-4000-8000-000000000000");
        fs::rename(package_dir(&instance, "tasks"), &orphan).unwrap();
        fs::create_dir_all(parent.join(".tmp-deadbeef")).unwrap();

        install(&instance, &tarball(&package("other", "0.1.0"), &[("dist/index.js", b"x")])).unwrap();
        assert!(package_dir(&instance, "tasks").exists(), "the orphaned copy was put back");
        assert!(!orphan.exists());
        assert!(!parent.join(".tmp-deadbeef").exists());
        assert!(leftover_work_dirs(&instance).is_empty());
    }

    #[test]
    fn every_refusal_says_why_and_leaves_nothing_behind() {
        let js = ("dist/index.js", &b"export {};\n"[..]);
        let cases: Vec<(Vec<u8>, &str)> = vec![
            (
                crate::plugins::testing::raw_tarball(&[Entry::symlink("package/dist/index.js", "../../../etc/passwd")]),
                "not a regular file or a directory",
            ),
            (
                crate::plugins::testing::raw_tarball(&[Entry::hardlink("package/dist/index.js", "package/package.json")]),
                "not a regular file or a directory",
            ),
            (
                crate::plugins::testing::raw_tarball(&[Entry::file("/etc/passwd", b"x")]),
                "is an absolute path",
            ),
            (
                crate::plugins::testing::raw_tarball(&[Entry::file("package/../escape.js", b"x")]),
                "steps out of the package",
            ),
            (crate::plugins::testing::raw_tarball(&[Entry::file("package/dist/index.js", b"x")]), "no package/package.json"),
            (tarball(&serde_json::json!({"version": "0.1.0"}), &[js]), "no \"droppen\" manifest"),
            (
                tarball(&with_manifest(serde_json::json!({"id": "Tasks", "engines": {"droppen": "^0.1.0"}})), &[js]),
                "is not a plugin id",
            ),
            (
                tarball(&with_version(serde_json::json!({"id": "tasks", "engines": {"droppen": "^0.1.0"}}), "1.0"), &[js]),
                "the droppen manifest is not one",
            ),
            (
                tarball(&with_manifest(serde_json::json!({"id": "tasks", "version": "0.1.0", "engines": {"droppen": "^0.1.0"}, "permissions": ["files:write"]})), &[js]),
                "unknown variant",
            ),
            (
                tarball(&with_manifest(serde_json::json!({"id": "tasks", "version": "0.1.0", "engines": {"droppen": "^0.1.0"}, "aliases": ["tasks"]})), &[js]),
                "one lower-case path segment",
            ),
            (
                tarball(&with_manifest(serde_json::json!({"id": "tasks", "version": "0.1.0", "engines": {"droppen": "^0.1.0"}, "aliases": ["/settings"]})), &[js]),
                "would shadow the shell's own settings",
            ),
            (
                tarball(&with_manifest(serde_json::json!({"id": "tasks", "version": "0.1.0", "engines": {"droppen": "^9.0.0"}})), &[js]),
                "needs Droppen plugin SDK ^9.0.0",
            ),
            (
                tarball(&entry_package("../outside.js"), &[js]),
                "steps out of the package",
            ),
            (
                tarball(&entry_package("/etc/passwd.js"), &[js]),
                "absolute",
            ),
            (tarball(&entry_package("dist/index.ts"), &[js]), "not a .js or .mjs file"),
            (tarball(&entry_package("dist/missing.js"), &[js]), "does not exist"),
            (
                crate::plugins::testing::raw_tarball(&[Entry::file("other/package.json", b"{}")]),
                "not under package/",
            ),
            (
                crate::plugins::testing::raw_tarball(&[Entry::file("package/package.json", b"{}"), Entry::file("package/package.json", b"{}")]),
                "appears twice",
            ),
            (
                crate::plugins::testing::raw_tarball(&[Entry::file("package/.droppen-install.json", b"{}")]),
                "reserved for the install's own record",
            ),
            (b"not a tarball at all".to_vec(), "not a gzip'd tar"),
            (
                crate::plugins::testing::raw_tarball(&[Entry::file_with_size("package/big.bin", MAX_UNPACKED_BYTES + 1, b"")]),
                "unpacks to more than",
            ),
            // Over the tarball cap: the check runs before the gzip is read.
            (vec![0u8; MAX_TARBALL_BYTES + 1], "over"),
        ];
        for (bytes, why) in cases {
            let message = refusal(&bytes);
            assert!(message.contains(why), "{why:?} not in {message:?}");
        }
    }

    #[test]
    fn more_than_two_thousand_files_is_refused() {
        let mut entries = vec![Entry::file("package/package.json", package("tasks", "0.1.0").to_string().into_bytes())];
        for index in 0..MAX_FILES + 1 {
            entries.push(Entry::file(format!("package/dist/{index}.js"), b"x".to_vec()));
        }
        let message = refusal(&crate::plugins::testing::raw_tarball(&entries));
        assert!(message.contains("more than 2000 files"), "{message}");
    }

    fn read_dir_names(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .map(|entries| entries.filter_map(Result::ok).map(|entry| entry.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default()
    }

    fn with_manifest(droppen: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "name": "@x/tasks", "version": "0.1.0", "droppen": droppen })
    }

    fn with_version(droppen: serde_json::Value, version: &str) -> serde_json::Value {
        serde_json::json!({ "name": "@x/tasks", "version": version, "droppen": droppen })
    }

    fn entry_package(entry: &str) -> serde_json::Value {
        serde_json::json!({
            "name": "@x/tasks",
            "version": "0.1.0",
            "module": entry,
            "droppen": {"id": "tasks", "engines": {"droppen": "^0.1.0"}}
        })
    }
}
