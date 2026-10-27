//! The plugin host: which plugins this instance has installed, which of them
//! it has on, where their code is served from, and each one's private document
//! store.
//!
//! # What a plugin is here
//!
//! A plugin is a front-end package installed into one instance at runtime
//! (`docs/plugins/runtime.md`): `plugin install <file.tgz>`, `POST
//! /api/plugins`, or Settings › Plugins. It ships no server code; the server
//! unpacks and checks it ([`install`]), keeps its documents behind the generic
//! routes in [`routes`], and serves its code to this instance's own pages from
//! `GET /plugins/<key>/<id>/<build>/<path>` ([`serve`]). The web build carries
//! no plugin code at all, and is the same build everywhere.
//!
//! # On or off
//!
//! Per instance, never per machine — two instances share one binary and one
//! machine-wide database, and a switch that flipped both would be the bug this
//! design exists to avoid. In order:
//!
//! 1. the configuration (`[plugins] disable` beats `enable`; see
//!    [`crate::config::PluginSettings`]), which also makes the switch read-only;
//! 2. the instance's own decision, a row in its `state.db` (`kv`, scope
//!    `plugins`), written by the settings page or the CLI — and by an install,
//!    which switches the plugin on;
//! 3. off.
//!
//! And only ever when the plugin's `engines.droppen` admits this host's SDK. The
//! decision is read on every call rather than cached, so `remote-hand-server
//! plugin enable` takes effect on a running server at its next request.
//!
//! # The capability key
//!
//! A page cannot send its bearer header with `import()`, and a query string
//! would not survive the plugin's own relative chunk imports, so the code's
//! URL carries an instance-wide secret as a path segment: 128 bits as hex in
//! the instance's `state.db`, made on first need and stable across restarts.
//! It reaches a page only inside [`droppen_plugin_sdk::PluginStatus::entry`],
//! and that only through a signed-in, non-cloud caller: the `/api/plugins…`
//! handlers and the asset route refuse a bridged (`cloud:`) identity, the
//! feed's own per-subscriber filter (`feed::global`) hands a `cloud:`
//! subscriber `plugins: []`, and `agent::bridge` refuses the plugin paths for
//! a device before they reach this server. The asset route compares the key
//! in constant time ([`Plugins::asset`]).
//!
//! # Leaving no trace
//!
//! A plugin that is not on never touches the disk: its store is opened lazily,
//! by the first call that passed [`Plugins::require_enabled`], and nothing else
//! opens it. A store whose file has gone (`plugin purge` moved it aside) is
//! forgotten and reopened rather than written through a handle to a moved file.

pub mod cli;
pub mod install;
pub mod routes;
pub mod serve;
pub mod store;
#[cfg(test)]
pub(crate) mod testing;
pub mod wire;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use droppen_plugin_sdk::{sdk_version, valid_id, Doc, DocMeta, DocWrite, Manifest, PluginStatus, PutDoc};
use serde::{Deserialize, Serialize};

use crate::db::scope;
use crate::feed::global::GlobalFeed;
use crate::sessions::Paths;
use install::{Installed, InstallError};
pub use store::{Store, StoreError};

/// The `state.db` row holding the capability key, in scope `plugins`.
///
/// A key no plugin id can name: ids are lower-case, and this one is not.
const ASSET_KEY_ROW: &str = "assetKey";

/// Who turned a plugin on or off, and through what.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionReason {
    /// The settings page (`PUT /api/plugins/{id}/enabled`).
    Settings,
    /// `remote-hand-server plugin enable|disable`.
    Cli,
    /// A decision written by the boot that moved a built-in feature into a
    /// plugin, which turned it on where it had data. Nothing writes it now, but
    /// the rows it wrote remain.
    Migrated,
    /// `plugin install` (or `POST /api/plugins`), which turns a plugin on.
    Install,
}

/// An instance's own decision about one plugin, as its `state.db` keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    /// On or off.
    pub enabled: bool,
    /// The signed-in user who decided, when there was one.
    pub by: Option<String>,
    /// When, ISO-8601.
    pub at: String,
    /// Through what.
    pub reason: DecisionReason,
}

/// Why a plugin call was refused.
#[derive(Debug, Clone, thiserror::Error)]
pub enum PluginError {
    /// No plugin of that id is installed on this instance (`404 plugin_not_found`).
    #[error("no plugin {0:?} is installed on this instance")]
    NotFound(String),
    /// Installed, but not on here, or incompatible (`404 plugin_disabled`).
    #[error("plugin {id:?} is not enabled on this instance{}", .reason.as_deref().map(|r| format!(": {r}")).unwrap_or_default())]
    Disabled {
        /// The plugin.
        id: String,
        /// Its status's `reason`.
        reason: Option<String>,
    },
    /// The configuration pins it; the switch is read-only (`409 forced_by_config`).
    #[error("the server configuration pins plugin {0:?} on or off; change [plugins] enable/disable (or APP_PLUGINS__ENABLE/DISABLE) instead")]
    Forced(String),
    /// The store refused or failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// An install or an uninstall was refused.
    #[error(transparent)]
    Install(#[from] InstallError),
}

/// A file of an installed plugin's, as the asset route serves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// The file's bytes.
    pub bytes: Vec<u8>,
    /// Its media type, by extension.
    pub content_type: &'static str,
}

/// The plugin host of one instance. One per server, in [`crate::api::AppState`].
pub struct Plugins {
    instance_dir: PathBuf,
    instance_db: PathBuf,
    force_on: BTreeSet<String>,
    force_off: BTreeSet<String>,
    feed: Option<Arc<GlobalFeed>>,
    stores: Mutex<HashMap<String, Arc<Store>>>,
    /// The capability key, once this process has read or made it.
    asset_key: Mutex<Option<String>>,
}

impl std::fmt::Debug for Plugins {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plugins")
            .field("instance_dir", &self.instance_dir)
            .field("installed", &self.installed().iter().map(|package| package.manifest.id.as_str()).collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Whether this request came over the cloud agent's bridge and speaks for a
/// device (`crate::auth` resolves it from the agent's headers). Nothing of the
/// plugin host is served to one.
pub(crate) fn is_cloud(who: &crate::auth::MaybeIdentity) -> bool {
    who.0.as_ref().is_some_and(|identity| identity.username.starts_with("cloud:"))
}

impl Plugins {
    /// A host for the instance whose directory and `state.db` are given.
    /// Touches nothing on disk.
    pub fn new(
        instance_dir: impl Into<PathBuf>,
        instance_db: impl Into<PathBuf>,
        config: &crate::config::PluginSettings,
        feed: Option<Arc<GlobalFeed>>,
    ) -> Self {
        Self {
            instance_dir: instance_dir.into(),
            instance_db: instance_db.into(),
            force_on: config.enabled(),
            force_off: config.disabled(),
            feed,
            stores: Mutex::new(HashMap::new()),
            asset_key: Mutex::new(None),
        }
    }

    /// The host of the instance `paths` names.
    pub fn load(paths: &Paths, config: &crate::config::PluginSettings, feed: Option<Arc<GlobalFeed>>) -> Arc<Self> {
        Arc::new(Self::new(paths.instance_dir(), paths.instance_db(), config, feed))
    }

    /// `<instance>/plugin-packages`: every installed package, by id.
    pub fn packages_dir(&self) -> PathBuf {
        install::packages_dir(&self.instance_dir)
    }

    /// `<instance>/plugin-packages/<id>` — the unpacked package.
    pub fn package_dir(&self, id: &str) -> PathBuf {
        install::package_dir(&self.instance_dir, id)
    }

    /// `<instance>/plugins/<id>` — everything of one plugin's on this instance.
    pub fn dir(&self, id: &str) -> PathBuf {
        self.instance_dir.join("plugins").join(id)
    }

    /// `<instance>/plugins/<id>/store.db`.
    pub fn store_path(&self, id: &str) -> PathBuf {
        self.dir(id).join("store.db")
    }

    /// The instance directory.
    pub fn instance_dir(&self) -> &Path {
        &self.instance_dir
    }

    /// Every package installed in this instance, by id.
    ///
    /// Read from disk on every call rather than cached: an install or an
    /// uninstall made by the CLI beside a running server must be seen by that
    /// server's next request, and the list is a handful of small files.
    pub fn installed(&self) -> Vec<Installed> {
        let dir = self.packages_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut installed = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            // Install temporaries start with `.`, which no plugin id may, so
            // they are passed over here without a word.
            let Some(id) = entry.file_name().to_str().filter(|id| valid_id(id)).map(str::to_string) else {
                continue;
            };
            match install::read_package(&entry.path()) {
                Ok(package) => installed.push(package),
                Err(error) => tracing::warn!(plugin = id, %error, "an installed plugin cannot be read; leaving it out"),
            }
        }
        installed.sort_by(|left, right| left.manifest.id.cmp(&right.manifest.id));
        installed
    }

    /// One installed package, if it is there and readable.
    pub fn installed_package(&self, id: &str) -> Option<Installed> {
        valid_id(id).then(|| install::read_package(&self.package_dir(id)).ok()).flatten()
    }

    /// One installed plugin's manifest.
    pub fn manifest(&self, id: &str) -> Option<Manifest> {
        self.installed_package(id).map(|package| package.manifest)
    }

    /// The instance's decision about `id`, if it ever made one.
    pub fn decision(&self, id: &str) -> Option<Decision> {
        let raw = crate::db::at(&self.instance_db).kv_get(scope::PLUGINS, id)?;
        serde_json::from_str(&raw)
            .map_err(|error| tracing::warn!(plugin = id, %error, "an unreadable plugin switch; treating it as never decided"))
            .ok()
    }

    fn record(&self, id: &str, decision: &Decision) {
        let value = serde_json::to_string(decision).expect("a decision serialises");
        crate::db::at(&self.instance_db).kv_set(scope::PLUGINS, id, &value);
    }

    /// Every installed plugin as this instance sees it, by id.
    pub fn status(&self) -> Vec<PluginStatus> {
        let installed = self.installed();
        if installed.is_empty() {
            // Nothing installed: not even the instance database is consulted.
            return Vec::new();
        }
        let statuses: Vec<PluginStatus> = installed.iter().map(|package| self.evaluate(package, None)).collect();
        // The key is made only for a plugin that will actually hand its code
        // to a page; a list of plugins that are all off needs none.
        if !statuses.iter().any(|status| status.enabled) {
            return statuses;
        }
        let key = self.asset_key();
        installed.iter().map(|package| self.evaluate(package, Some(&key))).collect()
    }

    /// [`Self::status`], off the async runtime: it reads the instance's
    /// database and lists a directory, and runs on every feed attach.
    pub async fn status_async(self: &Arc<Self>) -> Vec<PluginStatus> {
        let plugins = self.clone();
        tokio::task::spawn_blocking(move || plugins.status()).await.unwrap_or_else(|error| {
            tracing::error!(%error, "cannot read the plugins' status");
            Vec::new()
        })
    }

    /// One installed plugin as this instance sees it.
    pub fn status_of(&self, id: &str) -> Option<PluginStatus> {
        let package = self.installed_package(id)?;
        let status = self.evaluate(&package, None);
        if status.enabled {
            let key = self.asset_key();
            return Some(self.evaluate(&package, Some(&key)));
        }
        Some(status)
    }

    fn evaluate(&self, package: &Installed, key: Option<&str>) -> PluginStatus {
        let manifest = &package.manifest;
        let id = manifest.id.as_str();
        let mut status = effective(manifest, self.force_on.contains(id), self.force_off.contains(id), self.decision(id).as_ref(), &package.build);
        if status.enabled {
            if let Some(key) = key {
                status.entry = Some(entry_url(key, id, &package.build, &package.entry));
            }
        }
        status.notices = self.notice_list(id);
        status.notice = (!status.notices.is_empty()).then(|| status.notices.iter().map(|n| n.text.as_str()).collect::<Vec<_>>().join("；"));
        status
    }

    /// The instance's capability key, made on first need and remembered for
    /// the life of the process.
    fn asset_key(&self) -> String {
        {
            let cached = lock(&self.asset_key);
            if let Some(key) = cached.as_ref() {
                return key.clone();
            }
        }
        let db = crate::db::at(&self.instance_db);
        let key = match db.kv_get(scope::PLUGINS, ASSET_KEY_ROW).filter(|key| valid_key(key)) {
            Some(key) => key,
            None => {
                // Two first asks may both make a key; only one row can win.
                // `INSERT OR IGNORE` then read back makes both answers agree,
                // so every URL any client holds names the same secret.
                let generated = random_key();
                db.kv_set_if_absent(scope::PLUGINS, ASSET_KEY_ROW, &generated);
                match db.kv_get(scope::PLUGINS, ASSET_KEY_ROW).filter(|key| valid_key(key)) {
                    Some(key) => key,
                    None => {
                        // An unreadable row, or a read-only database: use what
                        // this process made and try to leave it behind.
                        db.kv_set(scope::PLUGINS, ASSET_KEY_ROW, &generated);
                        generated
                    }
                }
            }
        };
        *lock(&self.asset_key) = Some(key.clone());
        key
    }

    /// The key if one has already been made — never makes one. The asset route
    /// asks with this: a request cannot bring a key into existence.
    fn stored_asset_key(&self) -> Option<String> {
        {
            let cached = lock(&self.asset_key);
            if let Some(key) = cached.as_ref() {
                return Some(key.clone());
            }
        }
        if !self.instance_db.exists() {
            // No database, so no key was ever made; do not make one now.
            return None;
        }
        let key = crate::db::at(&self.instance_db).kv_get(scope::PLUGINS, ASSET_KEY_ROW).filter(|key| valid_key(key))?;
        *lock(&self.asset_key) = Some(key.clone());
        Some(key)
    }

    /// Whether `id` is installed, on here and compatible.
    pub fn is_enabled(&self, id: &str) -> bool {
        self.status_of(id).is_some_and(|status| status.enabled)
    }

    /// Turn `id` on or off for this instance. Refused for a plugin this
    /// instance has not installed, and for one the configuration pins.
    pub fn set_enabled(&self, id: &str, enabled: bool, by: Option<&str>, reason: DecisionReason) -> Result<PluginStatus, PluginError> {
        if self.installed_package(id).is_none() {
            return Err(PluginError::NotFound(id.to_string()));
        }
        if self.force_on.contains(id) || self.force_off.contains(id) {
            return Err(PluginError::Forced(id.to_string()));
        }
        self.record(
            id,
            &Decision {
                enabled,
                by: by.map(str::to_string),
                at: store::now(),
                reason,
            },
        );
        tracing::info!(plugin = id, enabled, ?by, ?reason, "plugin switched");
        self.announce_status();
        self.status_of(id).ok_or_else(|| PluginError::NotFound(id.to_string()))
    }

    /// Unpack and install `tarball` as this instance's copy of its plugin,
    /// replacing any previous install of the same id and switching it on
    /// (unless the configuration pins it off).
    pub fn install(&self, tarball: &[u8], by: Option<&str>) -> Result<PluginStatus, PluginError> {
        let installed = install::install(&self.instance_dir, tarball)?;
        let id = installed.manifest.id.as_str().to_string();
        if !self.force_on.contains(&id) && !self.force_off.contains(&id) {
            self.record(
                &id,
                &Decision {
                    enabled: true,
                    by: by.map(str::to_string),
                    at: store::now(),
                    reason: DecisionReason::Install,
                },
            );
        }
        tracing::info!(plugin = %id, build = %installed.build, version = %installed.manifest.version, ?by, "plugin installed");
        self.announce_status();
        self.status_of(&id).ok_or(PluginError::NotFound(id))
    }

    /// Remove the installed package of `id`. Its documents and its on/off
    /// decision stay for a later install.
    pub fn uninstall(&self, id: &str) -> Result<(), PluginError> {
        install::uninstall(&self.instance_dir, id)?;
        tracing::info!(plugin = id, "plugin uninstalled");
        self.announce_status();
        Ok(())
    }

    /// One file of an installed, enabled plugin at its current build, for the
    /// asset route — or `None` for every case that route answers `404`: a
    /// wrong key, an unknown id, an old build, a plugin that is off or
    /// incompatible, a path outside the package, a directory.
    pub fn asset(&self, key: &str, id: &str, build: &str, path: &str) -> Option<Asset> {
        let expected = self.stored_asset_key()?;
        if key.len() != expected.len() || !bool::from(subtle::ConstantTimeEq::ct_eq(key.as_bytes(), expected.as_bytes())) {
            return None;
        }
        if !valid_id(id) {
            return None;
        }
        let package = self.installed_package(id)?;
        if package.build != build {
            return None;
        }
        if !self.evaluate(&package, None).enabled {
            return None;
        }
        if install::is_record_path(path) {
            return None;
        }
        let dir = self.package_dir(id);
        let file = resolve_inside(&dir, path)?;
        // Resolve both through the filesystem, not just the last component: a
        // symlink in the middle of the path (installs never make one, but the
        // directory may have been changed by hand) must not reach outside the
        // package. `starts_with` is component-wise, so `/a/bc` is not under
        // `/a/b`.
        let dir = std::fs::canonicalize(&dir).ok()?;
        let file = std::fs::canonicalize(&file).ok()?;
        if !file.starts_with(&dir) || !std::fs::metadata(&file).is_ok_and(|meta| meta.is_file()) {
            return None;
        }
        // `is_record_path` compared ASCII case, but APFS folds Unicode case:
        // `.droppen-inſtall.json` (U+017F) names the record without matching
        // it. Comparing the file's identity with the record's catches that,
        // and every other alias of the file — a hard link included.
        if same_file(&file, &dir.join(install::RECORD_FILE)) {
            return None;
        }
        let bytes = std::fs::read(&file).ok()?;
        // The URL names one build's bytes and is cached immutable: if a swap
        // landed between the first look and this read, the record no longer
        // says `build`, and these bytes must not go out under that URL.
        if install::recorded_build(&dir).as_deref() != Some(build) {
            return None;
        }
        Some(Asset {
            bytes,
            content_type: content_type(path),
        })
    }

    /// The store of a plugin that is on — opened, and made, on first use — or
    /// the reason it is refused. Every `/api/plugins/{id}/…` call starts here.
    pub fn require_enabled(&self, id: &str) -> Result<Arc<Store>, PluginError> {
        let status = self.status_of(id).ok_or_else(|| PluginError::NotFound(id.to_string()))?;
        if !status.enabled {
            return Err(PluginError::Disabled {
                id: id.to_string(),
                reason: status.reason,
            });
        }
        Ok(self.open_store(id))
    }

    /// The store of `id`, whatever its switch says: for the CLI, which decides
    /// for itself whether to make one.
    ///
    /// The handle is cached, but only for as long as its file is there: one
    /// moved aside since (a purge) is forgotten, and the store made afresh.
    pub(crate) fn open_store(&self, id: &str) -> Arc<Store> {
        let path = self.store_path(id);
        let mut stores = lock(&self.stores);
        if let Some(store) = stores.get(id) {
            if path.exists() {
                return store.clone();
            }
            tracing::warn!(plugin = id, path = %path.display(), "the plugin's store has gone from disk; opening a fresh one");
            crate::db::forget(&path);
            stores.remove(id);
        } else if !path.exists() {
            // Another handle in this process may still name a file that a CLI
            // in another process moved away.
            crate::db::forget(&path);
        }
        let store = Arc::new(Store::open(&path));
        stores.insert(id.to_string(), store.clone());
        store
    }

    /// The store of `id` if it already exists on disk; never makes one.
    pub(crate) fn existing_store(&self, id: &str) -> Option<Arc<Store>> {
        self.store_path(id).exists().then(|| self.open_store(id))
    }

    /// One document.
    pub fn get(&self, id: &str, name: &str) -> Result<Doc, PluginError> {
        let store = self.require_enabled(id)?;
        store.get(name)?.ok_or_else(|| StoreError::DocNotFound(name.to_string()).into())
    }

    /// Every document under `prefix`, without values.
    pub fn list(&self, id: &str, prefix: &str) -> Result<Vec<DocMeta>, PluginError> {
        Ok(self.require_enabled(id)?.list(prefix)?)
    }

    /// One compare-and-set write.
    pub fn put(&self, id: &str, name: &str, write: PutDoc, by: Option<&str>) -> Result<Doc, PluginError> {
        let store = self.require_enabled(id)?;
        let doc = store.put(name, write.expected_revision, write.value, by)?;
        self.published(id, std::slice::from_ref(&doc));
        Ok(doc)
    }

    /// Several compare-and-set writes, all or none.
    pub fn batch(&self, id: &str, writes: Vec<DocWrite>, by: Option<&str>) -> Result<Vec<Doc>, PluginError> {
        let store = self.require_enabled(id)?;
        let docs = store.batch(writes, by)?;
        self.published(id, &docs);
        Ok(docs)
    }

    /// One page of a document's past versions, newest first.
    pub fn history(&self, id: &str, name: &str, offset: usize, limit: Option<usize>) -> Result<store::HistoryPage, PluginError> {
        Ok(self.require_enabled(id)?.history(name, offset, limit)?)
    }

    /// Tell every client which documents moved.
    pub(crate) fn published(&self, id: &str, docs: &[Doc]) {
        if let Some(feed) = &self.feed {
            for doc in docs {
                feed.publish(serde_json::json!({ "type": "plugin-doc", "plugin": id, "name": doc.name, "revision": doc.revision }).to_string());
            }
        }
    }

    /// Send every client the current list (`{"type":"plugins", …}`).
    pub fn announce_status(&self) {
        if let Some(feed) = &self.feed {
            feed.publish(serde_json::json!({ "type": "plugins", "plugins": self.status() }).to_string());
        }
    }

    fn notice_list(&self, id: &str) -> Vec<droppen_plugin_sdk::PluginNotice> {
        let mut list = Vec::new();
        if let Some(corpse) = reset_store(&self.dir(id)) {
            list.push(droppen_plugin_sdk::PluginNotice {
                id: "store-reset".into(),
                kind: "storeReset".into(),
                text: format!("存储已重置，旧文件在 {}", corpse.display()),
                at: String::new(),
                dismissible: false,
            });
        }
        list
    }
}

/// The newest `store.corrupt-<time>` a reset left beside a plugin's store.
fn reset_store(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stamp: u64 = name.strip_prefix("store.corrupt-")?.parse().ok()?;
            Some((stamp, entry.path()))
        })
        .max()
        .map(|(_, path)| path)
}

/// A plugin's switch, worked out: configuration, then the instance's decision,
/// then off — and on only when compatible.
pub fn effective(manifest: &Manifest, force_on: bool, force_off: bool, decision: Option<&Decision>, build: &str) -> PluginStatus {
    let host = sdk_version();
    let compatible = manifest.compatible_with(&host);
    let (wanted, why_off, pinned) = if force_off {
        (false, Some("disabled by the server configuration".to_string()), true)
    } else if force_on {
        (true, None, true)
    } else if let Some(decision) = decision {
        let by = decision.by.as_deref().map(|by| format!(" by {by}")).unwrap_or_default();
        (decision.enabled, Some(format!("turned off on this instance{by}")), false)
    } else {
        (false, Some("installed, and off until switched on for this instance".to_string()), false)
    };
    let reason = if !compatible {
        Some(format!(
            "needs Droppen plugin SDK {}; this host implements {host}",
            manifest.engines.droppen
        ))
    } else if !wanted {
        why_off
    } else if pinned {
        Some("enabled by the server configuration".to_string())
    } else {
        None
    };
    PluginStatus {
        id: manifest.id.clone(),
        version: manifest.version.clone(),
        name: manifest.name.clone(),
        enabled: wanted && compatible,
        switched_on: wanted,
        compatible,
        forced: pinned,
        reason,
        notice: None,
        notices: Vec::new(),
        build: build.to_string(),
        entry: None,
        aliases: manifest.aliases.clone(),
        permissions: manifest.permissions.clone(),
    }
}

/// Where a page imports an enabled plugin from: its entry's path inside the
/// package, percent-encoded segment by segment, under the capability key and
/// the build.
pub fn entry_url(key: &str, id: &str, build: &str, entry: &str) -> String {
    let path: Vec<String> = entry.split('/').map(percent_encode).collect();
    format!("/plugins/{key}/{id}/{build}/{}", path.join("/"))
}

/// The unreserved characters as RFC 3986 has them; everything else is
/// `%XX`. Package paths are normally plain, so this only matters for the few
/// a plugin could ship with a space or a `#`.
fn percent_encode(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => encoded.push(byte as char),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// A file path inside a package, or `None` for anything that is not one:
/// absolute, `.` or `..` segments, an empty segment, a backslash (a separator
/// on Windows), a NUL.
fn resolve_inside(dir: &Path, path: &str) -> Option<PathBuf> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains('\0') {
        return None;
    }
    let mut target = dir.to_path_buf();
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        target.push(part);
    }
    Some(target)
}

/// Whether two paths name the same file, by `(device, inode)` — so a Unicode
/// case alias on a case-folded filesystem, or a hard link under another name,
/// is the same file however different its path looks.
fn same_file(left: &Path, right: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(left), std::fs::metadata(right)) {
        (Ok(left), Ok(right)) => left.dev() == right.dev() && left.ino() == right.ino(),
        _ => false,
    }
}

/// The media type of a file inside a package, by extension, as §2 lists them.
fn content_type(path: &str) -> &'static str {
    let extension = path.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase());
    match extension.as_deref() {
        Some("js" | "mjs") => "text/javascript",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// 128 bits as 32 lower-case hex digits.
fn random_key() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Whether `key` is one this host could have made.
fn valid_key(key: &str) -> bool {
    key.len() == 32 && key.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::PluginSettings;
    use crate::plugins::testing::{package, tarball};
    use serde_json::json;

    /// A valid package of `id`, at `version`, with an entry and a chunk.
    pub(crate) fn package_bytes(id: &str, version: &str) -> Vec<u8> {
        tarball(
            &package(id, version),
            &[("dist/index.js", b"export default {};\n"), ("dist/page-abc.js", b"export const page = 1;\n")],
        )
    }

    pub(crate) fn host(dir: &Path, config: PluginSettings, feed: Option<Arc<GlobalFeed>>) -> Plugins {
        let instance = dir.join("instances").join("t");
        Plugins::new(instance.clone(), instance.join("state.db"), &config, feed)
    }

    pub(crate) fn installed_host(dir: &Path, ids: &[&str], config: PluginSettings, feed: Option<Arc<GlobalFeed>>) -> Plugins {
        let plugins = host(dir, config, feed);
        for id in ids {
            plugins.install(&package_bytes(id, "0.1.0"), None).expect("installs");
        }
        plugins
    }

    #[test]
    fn installing_switches_a_plugin_on_and_announces_it() {
        let dir = tempfile::tempdir().unwrap();
        let feed = GlobalFeed::new();
        let mut rx = feed.listen();
        let plugins = host(dir.path(), PluginSettings::default(), Some(feed.clone()));
        let status = plugins.install(&package_bytes("tasks", "0.1.0"), Some("ada")).unwrap();
        assert!(status.enabled && status.compatible && !status.forced);
        assert_eq!(status.version.to_string(), "0.1.0");
        assert!(status.build.starts_with("0.1.0-") && status.build.len() == 18);
        let entry = status.entry.expect("an enabled plugin has an entry");
        assert_eq!(entry, format!("/plugins/{}/tasks/{}/dist/index.js", status_key(&plugins), status.build));
        assert_eq!(status.aliases, ["/tasks"]);
        assert_eq!(status.permissions, [droppen_plugin_sdk::Permission::Storage]);
        let decision = plugins.decision("tasks").unwrap();
        assert_eq!((decision.enabled, decision.by.as_deref(), decision.reason), (true, Some("ada"), DecisionReason::Install));

        let frame: serde_json::Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(frame["type"], "plugins");
        assert_eq!(frame["plugins"][0]["build"], json!(status.build));
        assert_eq!(frame["plugins"][0]["entry"], json!(entry));
    }

    /// The key the status handed out, read back from the database rather than
    /// from the struct under test.
    fn status_key(plugins: &Plugins) -> String {
        let entry = plugins.status_of("tasks").and_then(|status| status.entry).expect("an entry");
        entry.strip_prefix("/plugins/").unwrap().split('/').next().unwrap().to_string()
    }

    #[test]
    fn the_capability_key_is_made_once_and_is_stable_for_the_instance() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = installed_host(dir.path(), &["tasks"], PluginSettings::default(), None);
        assert_eq!(status_key(&plugins), status_key(&plugins), "the same key twice");
        let first = status_key(&plugins);
        assert_eq!(first.len(), 32);
        assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));

        // Another process — the CLI, or a restarted server — reads the same row.
        let other = Plugins::new(plugins.instance_dir(), plugins.instance_dir().join("state.db"), &PluginSettings::default(), None);
        assert_eq!(status_key(&other), first, "the key survives the process");
    }

    #[test]
    fn a_plugin_that_is_off_has_no_entry_and_one_that_is_on_has_it() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = installed_host(dir.path(), &["tasks"], PluginSettings::default(), None);
        plugins.set_enabled("tasks", false, None, DecisionReason::Settings).unwrap();
        let status = plugins.status_of("tasks").unwrap();
        assert!(!status.enabled && status.entry.is_none(), "off: no URL to load");
        assert_eq!(status.reason.as_deref(), Some("turned off on this instance"));
        plugins.set_enabled("tasks", true, None, DecisionReason::Settings).unwrap();
        assert!(plugins.status_of("tasks").unwrap().entry.is_some());
    }

    #[test]
    fn an_instance_with_nothing_installed_has_nothing_and_touches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = host(dir.path(), PluginSettings::default(), None);
        assert!(plugins.status().is_empty());
        assert!(matches!(plugins.get("tasks", "state"), Err(PluginError::NotFound(_))));
        assert!(matches!(
            plugins.set_enabled("tasks", true, None, DecisionReason::Settings),
            Err(PluginError::NotFound(_))
        ));
        assert!(!dir.path().join("instances").exists(), "not even the instance database");
    }

    #[test]
    fn a_disabled_plugin_never_makes_a_store_and_a_switch_is_seen_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let feed = GlobalFeed::new();
        let plugins = installed_host(dir.path(), &["notes"], PluginSettings::default(), None);
        plugins.set_enabled("notes", false, Some("ada"), DecisionReason::Settings).unwrap();
        let mut rx = feed.listen();
        let switched = installed_host(dir.path(), &[], PluginSettings::default(), Some(feed.clone()));
        // The same instance directory, the switch the first host wrote.
        let store = switched.store_path("notes");
        assert!(matches!(switched.get("notes", "state"), Err(PluginError::Disabled { .. })));
        assert!(matches!(switched.list("notes", ""), Err(PluginError::Disabled { .. })));
        assert!(
            !store.exists() && !switched.dir("notes").exists(),
            "nothing on disk for a plugin that is off"
        );

        let status = switched.set_enabled("notes", true, Some("ada"), DecisionReason::Settings).unwrap();
        assert!(status.enabled);
        let frame: serde_json::Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(frame["type"], "plugins");
        assert_eq!(frame["plugins"][0]["enabled"], json!(true));
        let decision = switched.decision("notes").unwrap();
        assert_eq!((decision.by.as_deref(), decision.reason), (Some("ada"), DecisionReason::Settings));

        assert!(matches!(
            switched.get("notes", "state"),
            Err(PluginError::Store(StoreError::DocNotFound(_)))
        ));
        let doc = switched
            .put(
                "notes",
                "state",
                PutDoc {
                    expected_revision: None,
                    value: json!({"n": 1}),
                },
                Some("ada"),
            )
            .unwrap();
        assert_eq!(doc.revision, 1);
        let frame: serde_json::Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(frame, json!({"type": "plugin-doc", "plugin": "notes", "name": "state", "revision": 1}));
        assert!(store.exists());

        // Another process (the CLI) turns it off: the next call sees it.
        let other = host(dir.path(), PluginSettings::default(), None);
        other.set_enabled("notes", false, None, DecisionReason::Cli).unwrap();
        assert!(matches!(switched.get("notes", "state"), Err(PluginError::Disabled { .. })));
    }

    #[test]
    fn a_forced_plugin_cannot_be_switched_and_disable_wins() {
        let dir = tempfile::tempdir().unwrap();
        let config = PluginSettings {
            enable: Some("notes,clock".into()),
            disable: Some("clock".into()),
        };
        let plugins = installed_host(dir.path(), &["notes", "clock"], config, None);
        assert!(matches!(
            plugins.set_enabled("notes", false, None, DecisionReason::Settings),
            Err(PluginError::Forced(_))
        ));
        let status = plugins.status();
        assert_eq!(status.iter().map(|status| status.id.as_str()).collect::<Vec<_>>(), ["clock", "notes"], "by id");
        assert!(!status[0].enabled && status[0].forced);
        assert!(status[0].entry.is_none());
        assert!(status[1].enabled && status[1].forced);
        assert!(status[1].entry.is_some(), "a forced-on plugin is served");
        assert!(matches!(plugins.require_enabled("clock"), Err(PluginError::Disabled { .. })));
    }

    #[test]
    fn an_install_pinned_off_by_configuration_stays_off() {
        let dir = tempfile::tempdir().unwrap();
        let config = PluginSettings {
            enable: None,
            disable: Some("notes".into()),
        };
        let plugins = installed_host(dir.path(), &["notes"], config, None);
        let status = plugins.status_of("notes").unwrap();
        assert!(!status.enabled && status.forced && status.entry.is_none());
        assert!(plugins.decision("notes").is_none(), "a pinned plugin's install records no decision");
    }

    #[test]
    fn a_reinstall_keeps_the_store_and_an_uninstall_keeps_store_and_switch() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = installed_host(dir.path(), &["notes"], PluginSettings::default(), None);
        plugins
            .put(
                "notes",
                "state",
                PutDoc {
                    expected_revision: None,
                    value: json!(1),
                },
                None,
            )
            .unwrap();
        plugins.install(&package_bytes("notes", "0.2.0"), None).unwrap();
        assert_eq!(plugins.get("notes", "state").unwrap().value, json!(1), "a reinstall keeps the documents");
        plugins.set_enabled("notes", false, None, DecisionReason::Settings).unwrap();
        plugins.uninstall("notes").unwrap();
        assert!(!plugins.package_dir("notes").exists());
        assert!(plugins.store_path("notes").exists(), "uninstall removes the package only");
        assert_eq!(plugins.decision("notes").unwrap().enabled, false, "and keeps the switch");
        assert!(matches!(plugins.uninstall("notes"), Err(PluginError::Install(InstallError::NotFound(_)))));
    }

    #[test]
    fn the_asset_route_answers_only_for_this_key_build_switch_and_package() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = installed_host(dir.path(), &["tasks"], PluginSettings::default(), None);
        let status = plugins.status_of("tasks").unwrap();
        let key = status_key(&plugins);
        let build = status.build.clone();
        let url = status.entry.unwrap();
        let path = url
            .strip_prefix(&format!("/plugins/{key}/tasks/{build}/"))
            .expect("the entry URL")
            .to_string();
        assert_eq!(path, "dist/index.js");

        let asset = plugins.asset(&key, "tasks", &build, &path).expect("the entry");
        assert_eq!(asset.content_type, "text/javascript");
        assert_eq!(asset.bytes, b"export default {};\n");
        assert_eq!(
            plugins.asset(&key, "tasks", &build, "dist/page-abc.js").unwrap().content_type,
            "text/javascript"
        );
        assert_eq!(plugins.asset(&key, "tasks", &build, "package.json").unwrap().content_type, "application/json");

        let wrong = "0".repeat(32);
        assert!(plugins.asset(&wrong, "tasks", &build, &path).is_none(), "wrong key");
        assert!(plugins.asset(&key, "notes", &build, &path).is_none(), "unknown id");
        assert!(plugins.asset(&key, "tasks", "0.1.0-000000000000", &path).is_none(), "old build");
        for bad in ["../package.json", "dist/../package.json", "/etc/passwd", "", "dist//index.js", "dist\\index.js"] {
            assert!(plugins.asset(&key, "tasks", &build, bad).is_none(), "{bad:?}");
        }
        assert!(plugins.asset(&key, "tasks", &build, "dist").is_none(), "a directory");
        assert!(plugins.asset(&key, "tasks", &build, "dist/missing.js").is_none(), "a file that is not there");
        for record in [".droppen-install.json", ".DROPPEN-INSTALL.JSON", "dist/.droppen-install.json"] {
            assert!(plugins.asset(&key, "tasks", &build, record).is_none(), "the install record: {record}");
        }

        plugins.set_enabled("tasks", false, None, DecisionReason::Settings).unwrap();
        assert!(plugins.asset(&key, "tasks", &build, &path).is_none(), "a plugin that is off");
    }

    /// The record is the host's, not the package's: a Unicode case alias (APFS
    /// folds `ſ` to `s`) and a hard link under another name are both the same
    /// file, and both must be refused.
    #[test]
    fn the_install_record_is_refused_under_any_alias_of_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = installed_host(dir.path(), &["tasks"], PluginSettings::default(), None);
        let status = plugins.status_of("tasks").unwrap();
        let key = status_key(&plugins);
        let package = plugins.package_dir("tasks");

        assert!(
            plugins.asset(&key, "tasks", &status.build, ".droppen-inſtall.json").is_none(),
            "a Unicode case alias, on a filesystem that folds it"
        );
        // Every filesystem: the same file under another name.
        std::fs::hard_link(package.join(install::RECORD_FILE), package.join("dist/record-alias.json")).unwrap();
        assert!(
            plugins.asset(&key, "tasks", &status.build, "dist/record-alias.json").is_none(),
            "a hard link to the record"
        );
    }

    /// Installs never create symlinks, but a package directory changed by hand
    /// could; a path through one must not reach outside the package.
    #[test]
    fn an_asset_path_through_a_symlink_does_not_leave_the_package() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = installed_host(dir.path(), &["tasks"], PluginSettings::default(), None);
        let status = plugins.status_of("tasks").unwrap();
        let key = status_key(&plugins);
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"secret").unwrap();
        std::os::unix::fs::symlink(&outside, plugins.package_dir("tasks").join("dist").join("link")).unwrap();

        assert!(plugins.asset(&key, "tasks", &status.build, "dist/link/secret.txt").is_none());
        assert!(plugins.asset(&key, "tasks", &status.build, "dist/link").is_none());
    }

    /// The key is made by whichever caller asks first; two of them must not
    /// each make one. `INSERT OR IGNORE` + read-back is what makes them agree.
    #[test]
    fn two_first_asks_for_the_capability_key_agree() {
        let dir = tempfile::tempdir().unwrap();
        // Forced on, so no switch has to be written (and no status read has
        // made the key already) before the race.
        let config = PluginSettings {
            enable: Some("tasks".into()),
            disable: None,
        };
        let plugins = Arc::new(host(dir.path(), config, None));
        // Place the package without `Plugins::install`, which would make the
        // key itself.
        crate::plugins::install::install(plugins.instance_dir(), &package_bytes("tasks", "0.1.0")).unwrap();

        let (first, second) = (plugins.clone(), plugins.clone());
        let first = std::thread::spawn(move || first.status_of("tasks").unwrap().entry.unwrap());
        let second = std::thread::spawn(move || second.status_of("tasks").unwrap().entry.unwrap());
        assert_eq!(first.join().unwrap(), second.join().unwrap());
    }

    #[test]
    fn the_content_types_are_the_contract_s_own() {
        for (path, kind) in [
            ("a.js", "text/javascript"),
            ("a.mjs", "text/javascript"),
            ("a.css", "text/css"),
            ("a.json", "application/json"),
            ("a.svg", "image/svg+xml"),
            ("a.png", "image/png"),
            ("a.woff2", "font/woff2"),
            ("a.WOFF2", "font/woff2"),
            ("a.wasm", "application/wasm"),
            ("a.txt", "application/octet-stream"),
            ("a.tar.gz", "application/octet-stream"),
            ("no-extension", "application/octet-stream"),
        ] {
            assert_eq!(content_type(path), kind, "{path}");
        }
    }

    #[test]
    fn the_switch_is_config_then_decision_then_off() {
        let manifest: Manifest = serde_json::from_value(json!({
            "id": "notes",
            "version": "0.1.0",
            "name": "notes",
            "engines": {"droppen": "^0.1.0"},
            "permissions": ["storage"],
            "aliases": ["/notes"],
        }))
        .unwrap();
        let decided = |enabled| Decision {
            enabled,
            by: Some("ada".into()),
            at: "2026-10-17T00:00:00.000Z".into(),
            reason: DecisionReason::Settings,
        };
        let build = "0.1.0-0123456789ab";
        assert!(!effective(&manifest, false, false, None, build).enabled, "off until something says otherwise");
        assert!(effective(&manifest, false, false, Some(&decided(true)), build).enabled);
        assert!(!effective(&manifest, false, false, Some(&decided(false)), build).enabled);
        let pinned = effective(&manifest, true, false, Some(&decided(false)), build);
        assert!(pinned.enabled && pinned.forced, "the configuration beats the instance");
        assert!(pinned.entry.is_none(), "a switch cannot make the URL itself");
        let both = effective(&manifest, true, true, Some(&decided(true)), build);
        assert!(!both.enabled && both.forced, "disable beats enable");
        assert!(both.reason.is_some());
        assert_eq!(effective(&manifest, false, false, Some(&decided(true)), build).reason, None, "nothing to explain");

        let mut future = manifest.clone();
        future.engines.droppen = "^0.2.0".parse().unwrap();
        let status = effective(&future, true, false, None, build);
        assert!(!status.enabled && !status.compatible, "never on when the host cannot run it");
        assert!(status.switched_on, "but still wanted, so the page can say it needs an update");
        assert!(status.reason.unwrap().contains("^0.2.0"));
        assert!(effective(&future, false, false, Some(&decided(true)), build).switched_on);
        assert!(!effective(&future, false, false, Some(&decided(false)), build).switched_on, "switched off: nothing to say");
        assert!(!effective(&future, true, true, None, build).switched_on, "pinned off");
    }

    #[test]
    fn a_store_moved_away_is_reopened_fresh_and_a_reset_one_is_noticed() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = installed_host(dir.path(), &["notes"], PluginSettings::default(), None);
        let put = |expected| PutDoc {
            expected_revision: expected,
            value: json!(1),
        };
        plugins.put("notes", "a", put(None), None).unwrap();
        let aside = plugins.instance_dir().join("plugins").join("notes.purged-1");
        std::fs::rename(plugins.dir("notes"), &aside).unwrap();
        assert!(matches!(plugins.get("notes", "a"), Err(PluginError::Store(StoreError::DocNotFound(_)))));
        plugins.put("notes", "a", put(None), None).expect("a fresh store, not the moved file");
        assert!(plugins.store_path("notes").exists());
        assert_eq!(plugins.status_of("notes").unwrap().notice, None);

        std::fs::write(plugins.dir("notes").join("store.corrupt-17"), b"").unwrap();
        std::fs::write(plugins.dir("notes").join("store.corrupt-17-wal"), b"").unwrap();
        let notice = plugins.status_of("notes").unwrap().notice.expect("a notice");
        assert!(notice.contains("store.corrupt-17") && !notice.contains("-wal"), "{notice}");
    }

    #[test]
    fn entry_urls_encode_what_a_path_segment_may_not_carry() {
        assert_eq!(entry_url("k", "tasks", "1.0.0-abcdef012345", "dist/index.js"), "/plugins/k/tasks/1.0.0-abcdef012345/dist/index.js");
        assert_eq!(
            entry_url("k", "tasks", "b", "dist/a b#c.js"),
            "/plugins/k/tasks/b/dist/a%20b%23c.js"
        );
    }
}
