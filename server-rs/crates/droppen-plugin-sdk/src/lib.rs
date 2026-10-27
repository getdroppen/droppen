//! Droppen's plugin contract, Rust side.
//!
//! In SDK 0.1 no plugin ships Rust code: a plugin is a front-end package, and
//! the only server-side extension point is the host's generic document store.
//! This crate is therefore what the *host* links to speak that contract — the
//! manifest a plugin package declares, the rules for plugin ids and document
//! names, the engine-range check, and the store's wire format — written down
//! once, apart from the host, so the contract can be published and versioned on
//! its own (MIT OR Apache-2.0, whatever the host's licence).
//!
//! It names nothing of the host's (no web framework, no runtime, no database)
//! and nothing of the hosted service (accounts, plans, relays, keys). The
//! TypeScript half is `@droppen/plugin-sdk`; the two describe the same JSON.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The SDK version a host implements. A plugin's `engines.droppen` is a range over it.
pub const SDK_VERSION: &str = "0.1.0";

/// [`SDK_VERSION`], parsed.
pub fn sdk_version() -> semver::Version {
    semver::Version::parse(SDK_VERSION).expect("SDK_VERSION is a version")
}

/// Error codes the store answers with, in the `error` field of the body.
pub mod codes {
    /// The plugin is not enabled on this instance (or is incompatible).
    pub const PLUGIN_DISABLED: &str = "plugin_disabled";
    /// No plugin of that id is bundled with this build.
    pub const PLUGIN_NOT_FOUND: &str = "plugin_not_found";
    /// The document does not exist.
    pub const DOC_NOT_FOUND: &str = "doc_not_found";
    /// A write named a revision that is no longer the current one.
    pub const CONFLICT: &str = "conflict";
    /// A document, a request or the plugin's whole store is over its limit.
    pub const TOO_LARGE: &str = "too_large";
    /// A document name or body the store will not accept.
    pub const INVALID: &str = "invalid";
    /// The configuration pins the plugin on or off; the switch is read-only.
    pub const FORCED_BY_CONFIG: &str = "forced_by_config";
}

/// A plugin id: `^[a-z][a-z0-9-]{1,31}$`.
///
/// It becomes a URL (`/p/<id>`), an API prefix (`/api/plugins/<id>`), a
/// directory name and an i18n namespace, so anything else is refused at the
/// boundary — `serde` included — rather than escaped later.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PluginId(String);

/// A string that is not a plugin id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidId(pub String);

impl fmt::Display for InvalidId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} is not a plugin id: 2-32 characters of a-z, 0-9 or '-', starting with a letter",
            self.0
        )
    }
}

impl std::error::Error for InvalidId {}

/// Whether `id` is a valid plugin id.
pub fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (2..=32).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

impl PluginId {
    /// Check and wrap.
    pub fn new(id: impl Into<String>) -> Result<Self, InvalidId> {
        let id = id.into();
        if valid_id(&id) {
            Ok(Self(id))
        } else {
            Err(InvalidId(id))
        }
    }

    /// The id itself.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PluginId {
    type Error = InvalidId;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<PluginId> for String {
    fn from(id: PluginId) -> Self {
        id.0
    }
}

impl FromStr for PluginId {
    type Err = InvalidId;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for PluginId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Whether `name` is a usable document name: 1-200 characters of
/// `A-Z a-z 0-9 . _ -` in `/`-separated segments, none empty, `.` or `..`.
pub fn valid_doc_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && name.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
}

/// What a plugin asks the host for. Only `storage` is implemented in 0.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Permission {
    /// The plugin's document store.
    #[serde(rename = "storage")]
    Storage,
    /// Reserved: read session metadata.
    #[serde(rename = "sessions:read")]
    SessionsRead,
    /// Reserved: send notifications.
    #[serde(rename = "notify")]
    Notify,
}

/// One string, or one per language (`en` required as the fallback).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LocalizedText {
    /// The same in every language.
    Plain(String),
    /// Language tag (`en`, `zh-CN`, `zh-TW`, `ja`) to text.
    Localized(BTreeMap<String, String>),
}

impl LocalizedText {
    /// The text for `locale`, falling back to `zh-CN` for `zh-TW`, then to `en`.
    pub fn localize(&self, locale: &str) -> &str {
        match self {
            Self::Plain(text) => text,
            Self::Localized(map) => map
                .get(locale)
                .or_else(|| (locale == "zh-TW").then(|| map.get("zh-CN")).flatten())
                .or_else(|| map.get("en"))
                .map(String::as_str)
                .unwrap_or_default(),
        }
    }
}

/// The host versions a plugin works with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Engines {
    /// A range over [`SDK_VERSION`], e.g. `^0.1.0`.
    pub droppen: semver::VersionReq,
}

/// A plugin's manifest: the `droppen` field of its `package.json`, with the
/// package's `version`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// See [`PluginId`].
    pub id: PluginId,
    /// The package version.
    pub version: semver::Version,
    /// Display name.
    pub name: LocalizedText,
    /// The SDK range the plugin was built against.
    pub engines: Engines,
    /// What it asks for.
    #[serde(default)]
    pub permissions: Vec<Permission>,
    /// Old top-level paths the host redirects to `/p/<id>/`.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Enabled on an instance that has never decided. The distribution may override it.
    #[serde(default)]
    pub default_enabled: bool,
}

impl Manifest {
    /// Whether a host implementing `host_sdk` can load this plugin.
    pub fn compatible_with(&self, host_sdk: &semver::Version) -> bool {
        self.engines.droppen.matches(host_sdk)
    }

    /// Problems a host should refuse the manifest for, beyond what parsing checks.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if let LocalizedText::Localized(map) = &self.name {
            if !map.contains_key("en") {
                problems.push("name has no `en` text".to_string());
            }
        }
        for alias in &self.aliases {
            let ok = alias.len() <= 64
                && alias
                    .strip_prefix('/')
                    .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
            if !ok {
                problems.push(format!("alias {alias:?} must be one lower-case path segment such as \"/tasks\""));
            }
            if alias == "/p" {
                problems.push("alias \"/p\" would shadow every plugin".to_string());
            }
        }
        problems
    }
}

/// `dist/plugins/manifest.json`: which plugins a build bundles.
///
/// Written by the build (never by hand) and embedded with the web assets, so
/// the server learns what it ships without running any JavaScript.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundledManifest {
    /// The SDK version the build was checked against.
    #[serde(default)]
    pub sdk_version: Option<semver::Version>,
    /// The distribution's name (`public` for the open build).
    #[serde(default)]
    pub distribution: Option<String>,
    /// Every bundled plugin.
    #[serde(default)]
    pub plugins: Vec<Manifest>,
}

impl BundledManifest {
    /// Read `dist/plugins/manifest.json` one plugin at a time: an entry that
    /// does not parse (a bad id, range, version, permission or name) is left
    /// out and described in the second half of the answer, so one bad plugin
    /// never takes the others with it. Only a file that is not a JSON object
    /// with a `plugins` array is an error.
    pub fn from_json_lenient(bytes: &[u8]) -> Result<(Self, Vec<String>), String> {
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        let object = value.as_object().ok_or("the manifest is not a JSON object")?;
        let entries = match object.get("plugins") {
            None => Vec::new(),
            Some(serde_json::Value::Array(entries)) => entries.clone(),
            Some(_) => return Err("`plugins` is not an array".into()),
        };
        let mut problems = Vec::new();
        let sdk_version = match object.get("sdkVersion") {
            None | Some(serde_json::Value::Null) => None,
            Some(raw) => serde_json::from_value(raw.clone())
                .map_err(|error| problems.push(format!("sdkVersion: {error}")))
                .ok(),
        };
        let distribution = object.get("distribution").and_then(serde_json::Value::as_str).map(str::to_string);
        let mut plugins = Vec::new();
        for (index, entry) in entries.into_iter().enumerate() {
            let label = entry
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map_or_else(|| format!("plugins[{index}]"), |id| format!("plugins[{index}] ({id:?})"));
            match serde_json::from_value::<Manifest>(entry) {
                Ok(manifest) => plugins.push(manifest),
                Err(error) => problems.push(format!("{label}: {error}")),
            }
        }
        Ok((
            Self {
                sdk_version,
                distribution,
                plugins,
            },
            problems,
        ))
    }
}

/// A stored document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Doc<T = serde_json::Value> {
    /// See [`valid_doc_name`].
    pub name: String,
    /// Bumped by every write; the first write makes it 1.
    pub revision: u64,
    /// ISO-8601.
    pub updated_at: String,
    /// The plugin's JSON.
    pub value: T,
}

/// A document without its value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocMeta {
    /// See [`valid_doc_name`].
    pub name: String,
    /// The current revision.
    pub revision: u64,
    /// ISO-8601.
    pub updated_at: String,
}

/// The body of `PUT /api/plugins/{id}/docs/{name}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutDoc<T = serde_json::Value> {
    /// The revision the write is based on; `null` means "must not exist yet".
    pub expected_revision: Option<u64>,
    /// The new value.
    pub value: T,
}

/// One write in a batch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocWrite<T = serde_json::Value> {
    /// See [`valid_doc_name`].
    pub name: String,
    /// The revision the write is based on; `null` means "must not exist yet".
    pub expected_revision: Option<u64>,
    /// The new value.
    pub value: T,
}

/// The body of `POST /api/plugins/{id}/batch`: all of these, or none.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Batch<T = serde_json::Value> {
    /// In order; a name may appear once.
    pub writes: Vec<DocWrite<T>>,
}

/// The `409` body: which document moved, and what it is now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict<T = serde_json::Value> {
    /// Always [`codes::CONFLICT`].
    pub error: String,
    /// Human-readable.
    pub message: String,
    /// The document that conflicted.
    pub doc: String,
    /// That document as it is now; `None` when it does not exist.
    pub current: Option<Doc<T>>,
}

/// One past version of a document, from its history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocVersion<T = serde_json::Value> {
    /// The revision this value had.
    pub revision: u64,
    /// When it was written, ISO-8601.
    pub updated_at: String,
    /// Who wrote it, when the host knows.
    pub updated_by: Option<String>,
    /// The value at that revision.
    pub value: T,
}

/// A plugin as one instance sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginStatus {
    /// See [`PluginId`].
    pub id: PluginId,
    /// The bundled package version.
    pub version: semver::Version,
    /// Display name.
    pub name: LocalizedText,
    /// On for this instance (and compatible).
    pub enabled: bool,
    /// The host satisfies the plugin's engine range.
    pub compatible: bool,
    /// The configuration pins it on or off; the switch is read-only.
    pub forced: bool,
    /// Why it is off, incompatible or pinned, when it is.
    pub reason: Option<String>,
    /// Something the user should know that does not stop it working (today, a
    /// store that had to be reset): every notice's text, joined.
    /// [`Self::notices`] has them one by one.
    pub notice: Option<String>,
    /// Every notice, oldest first.
    #[serde(default)]
    pub notices: Vec<PluginNotice>,
}

/// One thing a plugin's host wants the user to see.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginNotice {
    /// Stable while the notice exists.
    pub id: String,
    /// What it is about (`storeReset`).
    pub kind: String,
    /// For people.
    pub text: String,
    /// When, ISO-8601.
    pub at: String,
    /// Whether a client may dismiss it. A store reset is not: it goes away when
    /// the user clears its file.
    pub dismissible: bool,
}

/// Everything a plugin has stored, as `plugin export` writes it and `plugin import` reads it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreExport {
    /// Always `droppen.plugin-store.v1`.
    pub format: String,
    /// Whose documents.
    pub plugin: PluginId,
    /// When, ISO-8601.
    pub exported_at: String,
    /// Every document, by name.
    pub docs: Vec<Doc>,
}

/// [`StoreExport::format`].
pub const EXPORT_FORMAT: &str = "droppen.plugin-store.v1";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plugin_ids_are_checked_at_the_boundary() {
        for id in ["tasks", "my-plugin", "a1"] {
            assert!(PluginId::new(id).is_ok(), "{id}");
        }
        for id in ["Tasks", "../x", "1abc", "a", "a_b", "", &"x".repeat(33)] {
            assert!(PluginId::new(id).is_err(), "{id}");
        }
        assert!(serde_json::from_value::<PluginId>(json!("Tasks")).is_err());
        assert!(serde_json::from_value::<PluginId>(json!("../x")).is_err());
        assert_eq!(serde_json::to_value(PluginId::new("tasks").unwrap()).unwrap(), json!("tasks"));
    }

    #[test]
    fn document_names_are_safe_segments() {
        for name in ["state", "events/2026-10", "reports/2026-10-12", "a.b_c-d"] {
            assert!(valid_doc_name(name), "{name}");
        }
        for name in ["", "/state", "state/", "a//b", "../x", "a/./b", "a b", "名", &"x".repeat(201)] {
            assert!(!valid_doc_name(name), "{name}");
        }
    }

    fn manifest() -> serde_json::Value {
        json!({
            "id": "tasks",
            "version": "0.1.0",
            "name": {"en": "Tasks", "zh-CN": "待办"},
            "engines": {"droppen": "^0.1.0"},
            "permissions": ["storage"],
            "aliases": ["/tasks"]
        })
    }

    #[test]
    fn a_manifest_round_trips_and_answers_compatibility() {
        let parsed: Manifest = serde_json::from_value(manifest()).expect("a manifest");
        assert!(!parsed.default_enabled, "plugins are off unless something says otherwise");
        assert_eq!(parsed.permissions, vec![Permission::Storage]);
        assert!(parsed.problems().is_empty());
        let again: Manifest = serde_json::from_value(serde_json::to_value(&parsed).unwrap()).unwrap();
        assert_eq!(again, parsed);
        assert!(parsed.compatible_with(&sdk_version()));
        assert!(parsed.compatible_with(&semver::Version::new(0, 1, 7)));
        assert!(!parsed.compatible_with(&semver::Version::new(0, 2, 0)), "every 0.x minor may break");
        assert_eq!(parsed.name.localize("zh-TW"), "待办");
        assert_eq!(parsed.name.localize("ja"), "Tasks");
    }

    #[test]
    fn bad_manifests_are_refused() {
        let mut bad = manifest();
        bad["id"] = json!("Tasks");
        assert!(serde_json::from_value::<Manifest>(bad).is_err());
        let mut bad = manifest();
        bad["engines"]["droppen"] = json!("not a range");
        assert!(serde_json::from_value::<Manifest>(bad).is_err());
        let mut odd = manifest();
        odd["aliases"] = json!(["tasks", "/a/b", "/p"]);
        odd["name"] = json!({"zh-CN": "待办"});
        let parsed: Manifest = serde_json::from_value(odd).unwrap();
        assert_eq!(parsed.problems().len(), 4, "{:?}", parsed.problems());
    }

    #[test]
    fn one_bad_plugin_in_a_bundled_manifest_leaves_the_others_standing() {
        let mut bad_range = manifest();
        bad_range["id"] = json!("notes");
        bad_range["engines"]["droppen"] = json!(">=0.1.0 <0.2.0 || ^0.3");
        let mut bad_permission = manifest();
        bad_permission["id"] = json!("clock");
        bad_permission["permissions"] = json!(["storage", "everything"]);
        let text = serde_json::to_vec(&json!({
            "sdkVersion": "0.1.0",
            "distribution": "mine",
            "plugins": [bad_range, manifest(), bad_permission, {"id": "Bad"}]
        }))
        .unwrap();
        let (bundled, problems) = BundledManifest::from_json_lenient(&text).unwrap();
        assert_eq!(bundled.plugins.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["tasks"]);
        assert_eq!(bundled.distribution.as_deref(), Some("mine"));
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[0].contains("notes"), "{problems:?}");
        assert!(BundledManifest::from_json_lenient(b"[1]").is_err());
        assert!(BundledManifest::from_json_lenient(b"{\"plugins\": 3}").is_err());
        assert_eq!(BundledManifest::from_json_lenient(b"{}").unwrap().0.plugins.len(), 0);
    }

    #[test]
    fn engine_ranges_read_as_cargo_reads_them() {
        // The build (web/build/droppen-plugins.ts) checks with these rules too:
        // a bare version is a caret requirement, comparators are joined by commas.
        let req = |text: &str| semver::VersionReq::parse(text);
        let v = |text: &str| semver::Version::parse(text).unwrap();
        assert!(req("0.1.0").unwrap().matches(&v("0.1.5")), "bare means caret");
        assert!(!req("0.1.0").unwrap().matches(&v("0.2.0")));
        assert!(req(">=0.1.0, <0.3.0").unwrap().matches(&v("0.2.9")));
        assert!(req(">=0.1.0 <0.3.0").is_err(), "space-separated comparators are npm's, not ours");
        assert!(req("^0.1 || ^0.2").is_err(), "so is ||");
    }

    /// The TypeScript SDK checks engine ranges at build time; the server reads
    /// them with this crate. Both run the same vectors, so they cannot disagree.
    #[test]
    fn engine_vectors_agree_with_the_typescript_sdk() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!("../../../../packages/plugin-sdk/test/engine-vectors.json")).unwrap();
        for case in vectors["matches"].as_array().unwrap() {
            let (range, version, expected) = (case[0].as_str().unwrap(), case[1].as_str().unwrap(), case[2].as_bool().unwrap());
            let req = semver::VersionReq::parse(range).unwrap_or_else(|e| panic!("{range:?}: {e}"));
            let version = semver::Version::parse(version).unwrap();
            assert_eq!(req.matches(&version), expected, "{range:?} against {version}");
        }
        for range in vectors["invalidRanges"].as_array().unwrap() {
            let range = range.as_str().unwrap();
            assert!(semver::VersionReq::parse(range).is_err(), "{range:?} should not parse");
        }
        for version in vectors["validVersions"].as_array().unwrap() {
            assert!(semver::Version::parse(version.as_str().unwrap()).is_ok(), "{version}");
        }
        for version in vectors["invalidVersions"].as_array().unwrap() {
            assert!(semver::Version::parse(version.as_str().unwrap()).is_err(), "{version}");
        }
    }

    #[test]
    fn wire_shapes_are_camel_case() {
        let put: PutDoc = serde_json::from_value(json!({"expectedRevision": null, "value": {"a": 1}})).unwrap();
        assert_eq!(put.expected_revision, None);
        let conflict = Conflict {
            error: codes::CONFLICT.into(),
            message: "moved".into(),
            doc: "state".into(),
            current: Some(Doc {
                name: "state".into(),
                revision: 3,
                updated_at: "2026-10-17T00:00:00.000Z".into(),
                value: json!({}),
            }),
        };
        let value = serde_json::to_value(&conflict).unwrap();
        assert_eq!(value["current"]["updatedAt"], json!("2026-10-17T00:00:00.000Z"));
        let status = PluginStatus {
            id: PluginId::new("tasks").unwrap(),
            version: semver::Version::new(0, 1, 0),
            name: LocalizedText::Plain("Tasks".into()),
            enabled: true,
            compatible: true,
            forced: false,
            reason: None,
            notice: None,
            notices: Vec::new(),
        };
        let value = serde_json::to_value(&status).unwrap();
        assert_eq!(value["version"], json!("0.1.0"));
        assert_eq!(value["reason"], json!(null));
    }
}
