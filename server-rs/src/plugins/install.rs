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
//! # The swap
//!
//! One version per plugin per instance. An install unpacks into a temporary
//! directory beside the final one, checks everything there, and only then
//! moves the old directory aside and the new one in. A refusal — anywhere
//! before that last step — removes the temporary directory and leaves the
//! previous install exactly as it was.
//!
//! # The install record
//!
//! The build id (`<version>-<first 12 hex of the tarball's sha256>`) names the
//! exact bytes that were installed, and no file inside the package records
//! it, so each install writes `.droppen-install.json` beside the package's
//! `package.json`. Reading an installed package back — for `GET /api/plugins`,
//! the feed, or the asset route — needs that record as well as
//! `package.json`; a directory without one is not an install, and is left
//! out with a warning rather than served.

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
/// The most regular files a package may hold.
pub const MAX_FILES: usize = 2_000;
/// A bound on every entry, directories included. A 32 MiB tarball can hold at
/// most 65,536 headers; this only keeps a directory-heavy one from turning
/// into that many syscalls before the file count has a chance to refuse it.
const MAX_ENTRIES: usize = 10_000;
/// Where an install writes its own record, inside the package directory.
const RECORD_FILE: &str = ".droppen-install.json";
/// The record's format tag.
const RECORD_FORMAT: &str = "droppen.plugin-install.v1";
/// What every entry of a `pnpm pack` tarball starts with.
const PREFIX: &str = "package/";

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

    swap_into_place(&temp, &package_dir(instance_dir, &id))?;
    guard.keep();

    Ok(Installed {
        manifest,
        build,
        entry,
    })
}

/// Remove an installed package. Its documents and its on/off decision stay.
pub fn uninstall(instance_dir: &Path, id: &str) -> Result<(), InstallError> {
    if !valid_id(id) {
        return Err(InstallError::NotFound(format!("no plugin {id:?} is installed")));
    }
    let dir = package_dir(instance_dir, id);
    match fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() => {}
        _ => return Err(InstallError::NotFound(format!("no plugin {id:?} is installed"))),
    }
    fs::remove_dir_all(&dir).map_err(|error| failed(format!("cannot remove {}: {error}", dir.display())))
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
    let manifest = manifest_of(&package)?;
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
    let decoder = flate2::read::GzDecoder::new(tarball);
    let mut archive = tar::Archive::new(decoder);
    let mut files = 0usize;
    let mut entries = 0usize;
    let mut unpacked = 0u64;
    let mut seen = std::collections::HashSet::new();

    for entry in archive.entries().map_err(|error| refused(format!("not a gzip'd tar: {error}")))? {
        let mut entry = entry.map_err(|error| refused(format!("not a gzip'd tar: {error}")))?;
        entries += 1;
        if entries > MAX_ENTRIES {
            return Err(refused(format!("more than {MAX_ENTRIES} entries")));
        }
        let raw = entry.path_bytes();
        let shown = String::from_utf8_lossy(&raw).into_owned();
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            return Err(refused(format!("{shown} is not a regular file or a directory")));
        }
        let path = package_path(shown.as_ref())?;
        if path.is_empty() {
            continue;
        }
        if path == RECORD_FILE {
            return Err(refused(format!("{shown} is reserved for the install's own record")));
        }
        if !seen.insert(path.clone()) {
            return Err(refused(format!("{shown} appears twice")));
        }
        let size = entry.size();
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

    let package_file = into.join("package.json");
    let bytes = fs::read(&package_file).map_err(|_| refused("the package has no package/package.json".to_string()))?;
    let package: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| refused(format!("package/package.json: {error}")))?;
    let manifest = manifest_of(&package).map_err(InstallError::Refused)?;
    if !manifest.compatible_with(&sdk_version()) {
        return Err(refused(format!(
            "it needs Droppen plugin SDK {}; this host implements {}",
            manifest.engines.droppen,
            sdk_version()
        )));
    }
    let entry = entry_of(&package, into).map_err(InstallError::Refused)?;
    Ok((manifest, entry))
}

/// One entry's path under the package directory, `/`-separated, or `""` for
/// the `package/` directory itself.
fn package_path(name: &str) -> Result<String, InstallError> {
    // `/` is the only separator an npm tarball uses; a backslash would be one
    // on Windows, where the same name would mean a different path.
    if name.contains('\\') || name.contains('\0') {
        return Err(refused(format!("{name:?} is not a path a package may carry")));
    }
    if name.starts_with('/') {
        return Err(refused(format!("{name:?} is an absolute path")));
    }
    let stripped = name.strip_prefix(PREFIX).ok_or_else(|| refused(format!("{name:?} is not under package/")))?;
    if stripped.is_empty() {
        return Ok(String::new());
    }
    let mut parts = Vec::new();
    for part in stripped.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(refused(format!("{name:?} steps out of the package"))),
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(refused(format!("{name:?} does not name a file inside the package")));
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

/// The manifest a package declares: its `droppen` object, with the package's
/// top-level `version` and, when the object gives no name, the id as one.
fn manifest_of(package: &serde_json::Value) -> Result<Manifest, String> {
    let object = package.as_object().ok_or("package/package.json is not a JSON object")?;
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

/// The entry module's relative path, resolved as the build resolves it:
/// `exports["."]` (a string, or its `import`/`default`), else `module`, else
/// `main`, else `index.js`; a relative `.js`/`.mjs` file that exists.
fn entry_of(package: &serde_json::Value, dir: &Path) -> Result<String, String> {
    let rel = entry_field(package)?;
    resolve_entry(dir, &rel)
}

/// The entry a package names, before it is checked: the build's `entryOf`,
/// nullish fallbacks and all. A name that is there but not a string is a
/// refusal rather than a reason to fall through to `index.js`.
fn entry_field(package: &serde_json::Value) -> Result<String, String> {
    let rel = match package.get("exports") {
        Some(serde_json::Value::String(rel)) => Some(serde_json::Value::String(rel.clone())),
        Some(exports @ serde_json::Value::Object(map)) => {
            // `exp["."] ?? exp`: an absent (or null) dot key means the object
            // itself is the dot entry, as it is in the build.
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
                .find_map(|key| package.get(*key).and_then(serde_json::Value::as_str))
                .unwrap_or("index.js")
                .to_string(),
        )
    });
    match rel {
        serde_json::Value::String(rel) => Ok(rel),
        _ => Err("cannot tell the entry module from \"exports\"".to_string()),
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

/// Move `temp` to `final_dir`, keeping the old directory until the new one is
/// in place. A failure restores the old one.
fn swap_into_place(temp: &Path, final_dir: &Path) -> Result<(), InstallError> {
    let parent = final_dir.parent().expect("a package directory has a parent");
    let old = parent.join(format!(".old-{}", uuid::Uuid::new_v4()));
    let had_old = match fs::symlink_metadata(final_dir) {
        Ok(_) => {
            fs::rename(final_dir, &old)
                .map_err(|error| failed(format!("cannot move {} aside: {error}", final_dir.display())))?;
            true
        }
        Err(_) => false,
    };
    match fs::rename(temp, final_dir) {
        Ok(()) => {
            if had_old {
                if let Err(error) = fs::remove_dir_all(&old) {
                    tracing::warn!(path = %old.display(), %error, "cannot remove the replaced plugin package");
                }
            }
            Ok(())
        }
        Err(error) => {
            if had_old {
                if let Err(restore) = fs::rename(&old, final_dir) {
                    tracing::error!(path = %final_dir.display(), %restore, "cannot put the previous plugin package back");
                }
            }
            Err(failed(format!("cannot move the new package into {}: {error}", final_dir.display())))
        }
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
    InstallError::Refused(message.into())
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
        assert!(
            !packages_dir(&instance).exists() || read_dir_names(&packages_dir(&instance)).is_empty(),
            "{message:?} left something behind: {:?}",
            read_dir_names(&packages_dir(&instance))
        );
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
        let leftovers: Vec<String> = fs::read_dir(packages_dir(&instance))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "no temporary directory outlived the install: {leftovers:?}");
    }

    #[test]
    fn uninstalling_removes_the_package_and_nothing_else() {
        let (_dir, instance) = temp();
        install(&instance, &tarball(&package("tasks", "0.1.0"), &[("dist/index.js", b"x")])).unwrap();
        uninstall(&instance, "tasks").unwrap();
        assert!(!package_dir(&instance, "tasks").exists());
        assert!(matches!(uninstall(&instance, "tasks"), Err(InstallError::NotFound(_))));
        assert!(matches!(uninstall(&instance, "../x"), Err(InstallError::NotFound(_))));
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
