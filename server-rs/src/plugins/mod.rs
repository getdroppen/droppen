//! The plugin host: which plugins this build bundles, which of them this
//! instance has on, and each one's private document store.
//!
//! # What a plugin is here
//!
//! In SDK 0.1 a plugin is a front-end package compiled into the web build
//! (see `docs/plugins/distribution.md`). It ships no server code; the one
//! thing the server does for it is keep its documents ([`store`]), behind the
//! generic routes in [`routes`] that `docs/plugins/storage-protocol.md`
//! specifies. The build writes `web/dist/plugins/manifest.json` saying which
//! plugins it bundles; that file is embedded with the rest of the web build
//! and is the only place this module learns of any plugin. The public build
//! bundles none, and then all of this is inert: an empty list, and every
//! `/api/plugins/{id}/…` a `404`.
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
//!    `plugins`), written by the settings page or the CLI;
//! 3. the distribution's `defaultEnabled`;
//! 4. off.
//!
//! And only ever when the plugin's `engines.droppen` admits this host's SDK. The
//! decision is read on every call rather than cached, so `remote-hand-server
//! plugin enable` takes effect on a running server at its next request.
//!
//! # Leaving no trace
//!
//! A plugin that is not on never touches the disk: its store is opened lazily,
//! by the first call that passed [`Plugins::require_enabled`], and nothing else
//! opens it. A store whose file has gone (`plugin purge` moved it aside) is
//! forgotten and reopened rather than written through a handle to a moved file.

pub mod cli;
pub mod routes;
pub mod store;
pub mod wire;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use droppen_plugin_sdk::{sdk_version, BundledManifest, Doc, DocMeta, DocWrite, Manifest, PluginStatus, PutDoc};
use serde::{Deserialize, Serialize};

use crate::db::scope;
use crate::feed::global::GlobalFeed;
use crate::sessions::Paths;
pub use store::{Store, StoreError};

/// Where the build puts the list of bundled plugins, under `web/dist`.
pub const MANIFEST_ASSET: &str = "plugins/manifest.json";

/// Who turned a plugin on or off, and through what.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionReason {
    /// The settings page (`PUT /api/plugins/{id}/enabled`).
    Settings,
    /// `remote-hand-server plugin enable|disable`.
    Cli,
    /// The first boot of the release that moved a built-in feature into a
    /// plugin, which turned it on where it had data. Kept for the decisions
    /// that release wrote; nothing writes it now.
    Migrated,
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
    /// No plugin of that id is bundled with this build (`404 plugin_not_found`).
    #[error("no plugin {0:?} is bundled with this build")]
    NotFound(String),
    /// Bundled, but not on here, or incompatible (`404 plugin_disabled`).
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
}

/// The bundled manifest of this binary's web build.
///
/// Absent — a build without a web bundle, or one made before plugins — means
/// no plugins. Present but unreadable means no plugins too, and a warning: the
/// server must not refuse to start over a file only the build writes.
pub fn embedded_manifest() -> BundledManifest {
    match crate::static_files::embedded(MANIFEST_ASSET) {
        None => BundledManifest::default(),
        Some(bytes) => read_manifest(&bytes),
    }
}

/// Read the bundled manifest one plugin at a time: an entry the server cannot
/// read is left out, loudly, and the others stand.
fn read_manifest(bytes: &[u8]) -> BundledManifest {
    match BundledManifest::from_json_lenient(bytes) {
        Ok((bundled, problems)) => {
            for problem in problems {
                tracing::error!(asset = MANIFEST_ASSET, %problem, "a bundled plugin's manifest cannot be read; that plugin is left out");
            }
            bundled
        }
        Err(error) => {
            tracing::error!(%error, asset = MANIFEST_ASSET, "the web build's plugin manifest cannot be read; serving no plugins");
            BundledManifest::default()
        }
    }
}

/// Drop every manifest a host must refuse: one with problems, or a second of an id.
fn admissible(bundled: BundledManifest) -> Vec<Manifest> {
    let mut seen = HashSet::new();
    let mut plugins = Vec::new();
    for manifest in bundled.plugins {
        let problems = manifest.problems();
        if !problems.is_empty() {
            tracing::warn!(plugin = %manifest.id, ?problems, "a bundled plugin's manifest is refused");
            continue;
        }
        if !seen.insert(manifest.id.clone()) {
            tracing::warn!(plugin = %manifest.id, "a plugin is bundled twice; keeping the first");
            continue;
        }
        plugins.push(manifest);
    }
    plugins
}

/// The plugin host of one instance. One per server, in [`crate::api::AppState`].
pub struct Plugins {
    instance_dir: PathBuf,
    instance_db: PathBuf,
    bundled: Vec<Manifest>,
    force_on: BTreeSet<String>,
    force_off: BTreeSet<String>,
    feed: Option<Arc<GlobalFeed>>,
    stores: Mutex<HashMap<String, Arc<Store>>>,
}

impl std::fmt::Debug for Plugins {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plugins")
            .field("instance_dir", &self.instance_dir)
            .field("bundled", &self.bundled.iter().map(|m| m.id.as_str()).collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Plugins {
    /// A host for the instance whose directory and `state.db` are given, with
    /// an explicit bundled manifest. Touches nothing on disk.
    pub fn new(
        instance_dir: impl Into<PathBuf>,
        instance_db: impl Into<PathBuf>,
        bundled: BundledManifest,
        config: &crate::config::PluginSettings,
        feed: Option<Arc<GlobalFeed>>,
    ) -> Self {
        Self {
            instance_dir: instance_dir.into(),
            instance_db: instance_db.into(),
            bundled: admissible(bundled),
            force_on: config.enabled(),
            force_off: config.disabled(),
            feed,
            stores: Mutex::new(HashMap::new()),
        }
    }

    /// The host of the instance `paths` names, with what this binary bundles.
    pub fn load(paths: &Paths, config: &crate::config::PluginSettings, feed: Option<Arc<GlobalFeed>>) -> Arc<Self> {
        let plugins = Self::new(paths.instance_dir(), paths.instance_db(), embedded_manifest(), config, feed);
        if !plugins.bundled.is_empty() {
            tracing::info!(
                plugins = ?plugins.bundled.iter().map(|m| format!("{}@{}", m.id, m.version)).collect::<Vec<_>>(),
                "bundled plugins"
            );
        }
        Arc::new(plugins)
    }

    /// Every plugin this build bundles, in the build's order.
    pub fn bundled(&self) -> &[Manifest] {
        &self.bundled
    }

    /// One bundled plugin's manifest.
    pub fn manifest(&self, id: &str) -> Option<&Manifest> {
        self.bundled.iter().find(|m| m.id.as_str() == id)
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

    /// Every bundled plugin as this instance sees it, in the build's order.
    pub fn status(&self) -> Vec<PluginStatus> {
        if self.bundled.is_empty() {
            // The public build: not even the instance database is consulted.
            return Vec::new();
        }
        let decisions = crate::db::at(&self.instance_db).kv_scope(scope::PLUGINS);
        self.bundled
            .iter()
            .map(|manifest| {
                let decision = decisions.get(manifest.id.as_str()).and_then(|raw| serde_json::from_str(raw).ok());
                self.evaluate(manifest, decision.as_ref())
            })
            .collect()
    }

    /// [`Self::status`], off the async runtime: it reads the instance's
    /// database and lists a directory, and runs on every feed attach.
    pub async fn status_async(self: &Arc<Self>) -> Vec<PluginStatus> {
        if self.bundled.is_empty() {
            return Vec::new();
        }
        let plugins = self.clone();
        tokio::task::spawn_blocking(move || plugins.status()).await.unwrap_or_else(|error| {
            tracing::error!(%error, "cannot read the plugins' status");
            Vec::new()
        })
    }

    /// One bundled plugin as this instance sees it.
    pub fn status_of(&self, id: &str) -> Option<PluginStatus> {
        let manifest = self.manifest(id)?;
        Some(self.evaluate(manifest, self.decision(id).as_ref()))
    }

    fn evaluate(&self, manifest: &Manifest, decision: Option<&Decision>) -> PluginStatus {
        let id = manifest.id.as_str();
        let mut status = effective(manifest, self.force_on.contains(id), self.force_off.contains(id), decision);
        status.notices = self.notice_list(id);
        status.notice = (!status.notices.is_empty()).then(|| status.notices.iter().map(|n| n.text.as_str()).collect::<Vec<_>>().join("；"));
        status
    }

    /// Whether `id` is bundled, on here and compatible.
    pub fn is_enabled(&self, id: &str) -> bool {
        self.status_of(id).is_some_and(|status| status.enabled)
    }

    /// Turn `id` on or off for this instance. Refused for a plugin this build
    /// does not bundle, and for one the configuration pins.
    pub fn set_enabled(&self, id: &str, enabled: bool, by: Option<&str>, reason: DecisionReason) -> Result<PluginStatus, PluginError> {
        if self.manifest(id).is_none() {
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
/// then the distribution's default, then off — and on only when compatible.
pub fn effective(manifest: &Manifest, force_on: bool, force_off: bool, decision: Option<&Decision>) -> PluginStatus {
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
        (
            manifest.default_enabled,
            Some("off unless turned on for this instance".to_string()),
            false,
        )
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
        compatible,
        forced: pinned,
        reason,
        notice: None,
        notices: Vec::new(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::PluginSettings;
    use serde_json::json;

    pub(crate) fn manifest(id: &str, default_enabled: bool) -> Manifest {
        serde_json::from_value(json!({
            "id": id,
            "version": "1.0.0",
            "name": {"en": id},
            "engines": {"droppen": "^0.1.0"},
            "permissions": ["storage"],
            "defaultEnabled": default_enabled,
            "package": format!("@droppen/plugin-{id}"),
        }))
        .expect("a manifest")
    }

    pub(crate) fn bundle(plugins: Vec<Manifest>) -> BundledManifest {
        BundledManifest {
            sdk_version: Some(sdk_version()),
            distribution: Some("test".into()),
            plugins,
        }
    }

    pub(crate) fn host(dir: &Path, plugins: Vec<Manifest>, config: PluginSettings, feed: Option<Arc<GlobalFeed>>) -> Plugins {
        let instance = dir.join("instances").join("t");
        Plugins::new(instance.clone(), instance.join("state.db"), bundle(plugins), &config, feed)
    }

    #[test]
    fn one_unreadable_entry_in_the_bundled_manifest_drops_only_itself() {
        let good = serde_json::to_value(manifest("tasks", false)).unwrap();
        let mut bad = good.clone();
        bad["id"] = json!("notes");
        bad["engines"]["droppen"] = json!(">=0.1.0 <0.2.0");
        let text = serde_json::to_vec(&json!({ "distribution": "mine", "plugins": [bad, good] })).unwrap();
        let bundled = read_manifest(&text);
        assert_eq!(bundled.plugins.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["tasks"]);
        assert!(read_manifest(b"not json").plugins.is_empty());
    }

    #[test]
    fn the_switch_is_config_then_decision_then_default_then_off() {
        let off = manifest("notes", false);
        let on = manifest("clock", true);
        let decided = |enabled| Decision {
            enabled,
            by: Some("ada".into()),
            at: "2026-10-17T00:00:00.000Z".into(),
            reason: DecisionReason::Settings,
        };
        assert!(!effective(&off, false, false, None).enabled, "off unless something says otherwise");
        assert!(effective(&on, false, false, None).enabled, "the distribution's default");
        assert!(
            !effective(&on, false, false, Some(&decided(false))).enabled,
            "the instance beats the default"
        );
        assert!(effective(&off, false, false, Some(&decided(true))).enabled);
        let pinned = effective(&off, true, false, Some(&decided(false)));
        assert!(pinned.enabled && pinned.forced, "the configuration beats the instance");
        let both = effective(&on, true, true, Some(&decided(true)));
        assert!(!both.enabled && both.forced, "disable beats enable");
        assert!(both.reason.is_some());
        assert_eq!(effective(&off, false, false, Some(&decided(true))).reason, None, "nothing to explain");

        let mut future = manifest("later", true);
        future.engines.droppen = "^0.2.0".parse().unwrap();
        let status = effective(&future, true, false, None);
        assert!(!status.enabled && !status.compatible, "never on when the host cannot run it");
        assert!(status.reason.unwrap().contains("^0.2.0"));
    }

    #[test]
    fn manifests_with_problems_or_a_repeated_id_are_dropped() {
        let mut odd = manifest("odd", false);
        odd.aliases = vec!["no-slash".into()];
        let dir = tempfile::tempdir().unwrap();
        let plugins = host(
            dir.path(),
            vec![manifest("tasks", false), odd, manifest("tasks", true)],
            PluginSettings::default(),
            None,
        );
        assert_eq!(plugins.bundled().iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["tasks"]);
        assert!(!plugins.bundled()[0].default_enabled, "the first of a repeated id is kept");
    }

    #[test]
    fn a_public_build_has_nothing_and_touches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = host(dir.path(), vec![], PluginSettings::default(), None);
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
        let mut rx = feed.listen();
        let plugins = host(dir.path(), vec![manifest("notes", false)], PluginSettings::default(), Some(feed.clone()));
        let store = plugins.store_path("notes");
        assert!(matches!(plugins.get("notes", "state"), Err(PluginError::Disabled { .. })));
        assert!(matches!(plugins.list("notes", ""), Err(PluginError::Disabled { .. })));
        assert!(
            !store.exists() && !plugins.dir("notes").exists(),
            "nothing on disk for a plugin that is off"
        );

        let status = plugins.set_enabled("notes", true, Some("ada"), DecisionReason::Settings).unwrap();
        assert!(status.enabled);
        let frame: serde_json::Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(frame["type"], "plugins");
        assert_eq!(frame["plugins"][0]["enabled"], json!(true));
        let decision = plugins.decision("notes").unwrap();
        assert_eq!((decision.by.as_deref(), decision.reason), (Some("ada"), DecisionReason::Settings));

        assert!(matches!(
            plugins.get("notes", "state"),
            Err(PluginError::Store(StoreError::DocNotFound(_)))
        ));
        let doc = plugins
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
        let other = host(dir.path(), vec![manifest("notes", false)], PluginSettings::default(), None);
        other.set_enabled("notes", false, None, DecisionReason::Cli).unwrap();
        assert!(matches!(plugins.get("notes", "state"), Err(PluginError::Disabled { .. })));
    }

    #[test]
    fn a_forced_plugin_cannot_be_switched_and_disable_wins() {
        let dir = tempfile::tempdir().unwrap();
        let config = PluginSettings {
            enable: Some("notes,clock".into()),
            disable: Some("clock".into()),
        };
        let plugins = host(dir.path(), vec![manifest("notes", false), manifest("clock", true)], config, None);
        assert!(matches!(
            plugins.set_enabled("notes", false, None, DecisionReason::Settings),
            Err(PluginError::Forced(_))
        ));
        let status = plugins.status();
        assert!(status[0].enabled && status[0].forced);
        assert!(!status[1].enabled && status[1].forced);
        assert!(matches!(plugins.require_enabled("clock"), Err(PluginError::Disabled { .. })));
    }

    #[test]
    fn a_store_moved_away_is_reopened_fresh_and_a_reset_one_is_noticed() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = host(dir.path(), vec![manifest("notes", true)], PluginSettings::default(), None);
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
}
