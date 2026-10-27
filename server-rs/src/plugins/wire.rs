//! The store protocol's shapes as the OpenAPI document describes them.
//!
//! `droppen-plugin-sdk` owns the wire types, and may not depend on gotcha, so
//! it cannot derive `Schematic`. These mirrors can; each serialises to exactly
//! the JSON its SDK twin does (`every_mirror_writes_the_json_its_sdk_twin_does`
//! pins it) and converts from it, so the handlers work in SDK types and only
//! the edge speaks these.
//!
//! Every name here starts with `Plugin`: gotcha's component registry is flat
//! and keyed by the Rust identifier, and a bare `Doc` would be one collision
//! away from silently describing something else.

use droppen_plugin_sdk as sdk;
use gotcha::axum::response::IntoResponse;
use gotcha::Schematic;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::openapi_ext::Nullable;

/// A display name: one string, or one per language with `en` required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PluginName(pub sdk::LocalizedText);

impl Schematic for PluginName {
    fn name() -> &'static str {
        "PluginName"
    }

    fn required() -> bool {
        true
    }

    fn type_() -> &'static str {
        "string"
    }

    fn generate_schema() -> gotcha_core::EnhancedSchema {
        let mut schema = gotcha::oas::Schema {
            _type: None,
            format: None,
            nullable: None,
            description: Some("One string, or one per language (`en`, `zh-CN`, `zh-TW`, `ja`) with `en` required.".to_string()),
            extras: Default::default(),
        };
        schema.extras.insert(
            "oneOf".to_string(),
            serde_json::json!([
                { "type": "string" },
                {
                    "type": "object",
                    "properties": { "en": { "type": "string" } },
                    "required": ["en"],
                    "additionalProperties": { "type": "string" }
                }
            ]),
        );
        gotcha_core::EnhancedSchema { schema, required: true }
    }
}

/// A plugin as this instance sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct PluginStatus {
    /// `^[a-z][a-z0-9-]{1,31}$`.
    pub id: String,
    /// The installed package version.
    pub version: String,
    /// Display name.
    pub name: PluginName,
    /// On for this instance, and compatible.
    pub enabled: bool,
    /// Switched on (by the configuration or this instance), whether or not
    /// this host can run it: with `compatible: false`, a plugin a host
    /// upgrade left behind, which the page names rather than drops.
    pub switched_on: bool,
    /// The host satisfies the plugin's `engines.droppen`.
    pub compatible: bool,
    /// The configuration pins it on or off; the switch is read-only.
    pub forced: bool,
    /// Why it is off, incompatible or pinned, when it is.
    pub reason: Nullable<String>,
    /// A problem that does not stop it working: today, a store that had to be
    /// reset. Every notice's text, joined.
    pub notice: Nullable<String>,
    /// Every notice, oldest first.
    pub notices: Vec<PluginNotice>,
    /// `<version>-<first 12 hex of the tarball's sha256>`: this exact code.
    pub build: String,
    /// Where a page imports the plugin from, when it is enabled and
    /// compatible (`/plugins/<key>/<id>/<build>/<entry>`); `null` otherwise.
    pub entry: Nullable<String>,
    /// The old top-level paths the host redirects to `/p/<id>/`.
    pub aliases: Vec<String>,
    /// What it asks the host for (`storage`, `sessions:read`, `notify`).
    pub permissions: Vec<String>,
}

/// One thing a plugin's host wants the user to see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct PluginNotice {
    /// Stable while the notice exists.
    pub id: String,
    /// What it is about (`storeReset`).
    pub kind: String,
    /// For people.
    pub text: String,
    /// When, ISO-8601.
    pub at: String,
    /// Whether a client may dismiss it. Kept for the wire contract; a store
    /// reset goes away when the user clears its file.
    pub dismissible: bool,
}

impl From<sdk::PluginNotice> for PluginNotice {
    fn from(notice: sdk::PluginNotice) -> Self {
        Self {
            id: notice.id,
            kind: notice.kind,
            text: notice.text,
            at: notice.at,
            dismissible: notice.dismissible,
        }
    }
}

impl From<sdk::PluginStatus> for PluginStatus {
    fn from(status: sdk::PluginStatus) -> Self {
        Self {
            id: status.id.to_string(),
            version: status.version.to_string(),
            name: PluginName(status.name),
            enabled: status.enabled,
            switched_on: status.switched_on,
            compatible: status.compatible,
            forced: status.forced,
            reason: status.reason.into(),
            notice: status.notice.into(),
            notices: status.notices.into_iter().map(Into::into).collect(),
            build: status.build,
            entry: status.entry.into(),
            aliases: status.aliases,
            permissions: status.permissions.into_iter().map(|permission| permission.as_str().to_string()).collect(),
        }
    }
}

/// `GET /api/plugins`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
pub struct PluginList {
    /// Every plugin installed in this instance.
    pub plugins: Vec<PluginStatus>,
}

/// The `400` of `POST /api/plugins`: the package was refused, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct PluginInvalidPackage {
    /// Always `invalid_package`.
    pub error: String,
    /// Human-readable; says which rule the package broke.
    pub message: String,
}

/// The body of `POST /api/plugins`: the package's bytes, gzip'd tar.
///
/// A schema for the OpenAPI document only — the handler reads raw bytes.
pub struct PluginTarballBody;

impl Schematic for PluginTarballBody {
    fn name() -> &'static str {
        "PluginTarballBody"
    }

    fn required() -> bool {
        true
    }

    fn type_() -> &'static str {
        "string"
    }

    fn generate_schema() -> gotcha_core::EnhancedSchema {
        let schema = gotcha::oas::Schema {
            _type: Some("string".to_string()),
            format: Some("binary".to_string()),
            nullable: None,
            description: Some("The plugin package: a gzip'd tar whose entries are under `package/`.".to_string()),
            extras: Default::default(),
        };
        gotcha_core::EnhancedSchema { schema, required: true }
    }
}

/// The empty answer to `DELETE /api/plugins/{id}`: axum sends `204`, and the
/// empty-body machinery documents it without a schema.
pub struct PluginDeleted;

impl IntoResponse for PluginDeleted {
    fn into_response(self) -> gotcha::axum::response::Response {
        gotcha::axum::http::StatusCode::NO_CONTENT.into_response()
    }
}

impl Schematic for PluginDeleted {
    fn name() -> &'static str {
        "void"
    }

    fn required() -> bool {
        false
    }

    fn type_() -> &'static str {
        "void"
    }

    fn empty_body() -> bool {
        true
    }
}

/// The body of `PUT /api/plugins/{id}/enabled`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct PluginSwitch {
    /// On or off, for this instance.
    pub enabled: bool,
}

/// A stored document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct PluginDoc {
    /// The document's name.
    pub name: String,
    /// Bumped by every write; the first write makes it 1.
    pub revision: u64,
    /// ISO-8601.
    pub updated_at: String,
    /// The plugin's JSON; the host never interprets it.
    pub value: Value,
}

impl From<sdk::Doc> for PluginDoc {
    fn from(doc: sdk::Doc) -> Self {
        Self {
            name: doc.name,
            revision: doc.revision,
            updated_at: doc.updated_at,
            value: doc.value,
        }
    }
}

/// The document a conflict names, as it is now.
///
/// The same JSON as [`PluginDoc`], under a name of its own: the `409` may
/// carry `null` here, and the OpenAPI pass that spells a nullable reference
/// marks the *component* nullable (`openapi_ext::hoist_nullable_refs`), which
/// would make every `PluginDoc` read as possibly `null`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct PluginCurrentDoc {
    /// The document's name.
    pub name: String,
    /// Its current revision.
    pub revision: u64,
    /// ISO-8601.
    pub updated_at: String,
    /// The plugin's JSON.
    pub value: Value,
}

impl From<sdk::Doc> for PluginCurrentDoc {
    fn from(doc: sdk::Doc) -> Self {
        Self {
            name: doc.name,
            revision: doc.revision,
            updated_at: doc.updated_at,
            value: doc.value,
        }
    }
}

/// A document without its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct PluginDocMeta {
    /// The document's name.
    pub name: String,
    /// The current revision.
    pub revision: u64,
    /// ISO-8601.
    pub updated_at: String,
}

impl From<sdk::DocMeta> for PluginDocMeta {
    fn from(meta: sdk::DocMeta) -> Self {
        Self {
            name: meta.name,
            revision: meta.revision,
            updated_at: meta.updated_at,
        }
    }
}

/// `GET /api/plugins/{id}/docs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct PluginDocList {
    /// By name.
    pub docs: Vec<PluginDocMeta>,
}

/// The query of `GET /api/plugins/{id}/docs`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Schematic)]
pub struct PluginDocQuery {
    /// Only documents whose name starts with this; every document when absent.
    pub prefix: Option<String>,
}

/// The body of `PUT /api/plugins/{id}/docs/{name}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct PluginPutDoc {
    /// The revision the write is based on; `null` means "must not exist yet".
    /// (Read leniently: a body without it means `null`, as the SDK's does.)
    pub expected_revision: Nullable<u64>,
    /// The new value.
    pub value: Value,
}

impl From<PluginPutDoc> for sdk::PutDoc {
    fn from(put: PluginPutDoc) -> Self {
        Self {
            expected_revision: put.expected_revision.0,
            value: put.value,
        }
    }
}

/// One write of a batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct PluginDocWrite {
    /// The document's name.
    pub name: String,
    /// The revision the write is based on; `null` means "must not exist yet".
    /// (Read leniently: a body without it means `null`, as the SDK's does.)
    pub expected_revision: Nullable<u64>,
    /// The new value.
    pub value: Value,
}

impl From<PluginDocWrite> for sdk::DocWrite {
    fn from(write: PluginDocWrite) -> Self {
        Self {
            name: write.name,
            expected_revision: write.expected_revision.0,
            value: write.value,
        }
    }
}

/// The body of `POST /api/plugins/{id}/batch`: all of these, or none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
pub struct PluginBatch {
    /// In order; a name may appear once.
    pub writes: Vec<PluginDocWrite>,
}

/// The answer to a batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
pub struct PluginBatchResult {
    /// Every document written, in the order of the writes.
    pub docs: Vec<PluginDoc>,
}

/// The `409`: which document moved, and what it is now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
pub struct PluginDocConflict {
    /// Always `conflict`.
    pub error: String,
    /// Human-readable.
    pub message: String,
    /// The first document whose revision did not hold.
    pub doc: String,
    /// That document as it is now; `null` when it does not exist.
    pub current: Nullable<PluginCurrentDoc>,
}

/// The `413`: which limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct PluginTooLarge {
    /// Always `too_large`.
    pub error: String,
    /// Human-readable.
    pub message: String,
    /// The limit that was passed, in bytes.
    pub limit: u64,
}

/// One past version of a document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct PluginDocVersion {
    /// The revision this value had.
    pub revision: u64,
    /// When it was written, ISO-8601.
    pub updated_at: String,
    /// Who wrote it, when the host knows.
    pub updated_by: Nullable<String>,
    /// The value at that revision.
    pub value: Value,
}

impl From<sdk::DocVersion> for PluginDocVersion {
    fn from(version: sdk::DocVersion) -> Self {
        Self {
            revision: version.revision,
            updated_at: version.updated_at,
            updated_by: version.updated_by.into(),
            value: version.value,
        }
    }
}

/// `GET /api/plugins/{id}/docs/{name}/history`: one page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
pub struct PluginDocHistory {
    /// Newest first: `limit` of them (ten unless asked, at most fifty), fewer
    /// when their values pass 8 MiB together (the first is always there).
    pub versions: Vec<PluginDocVersion>,
    /// Older versions remain; ask again with `offset` moved past these.
    pub more: bool,
}

/// Query for `GET /api/plugins/{id}/docs/{name}/history`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
pub struct PluginHistoryQuery {
    /// How many of the newest versions to skip.
    pub offset: Option<u32>,
    /// How many to answer with: 1 to 50, ten when absent.
    pub limit: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn same<A: Serialize, B: Serialize>(sdk: &A, mirror: &B) {
        assert_eq!(serde_json::to_value(sdk).unwrap(), serde_json::to_value(mirror).unwrap());
    }

    #[test]
    fn every_mirror_writes_the_json_its_sdk_twin_does() {
        for name in [
            sdk::LocalizedText::Plain("Tasks".into()),
            sdk::LocalizedText::Localized([("en".to_string(), "Tasks".to_string()), ("zh-CN".into(), "待办".into())].into()),
        ] {
            for (reason, notice) in [(None, None), (Some("off".to_string()), Some("reset".to_string()))] {
                let status = sdk::PluginStatus {
                    id: sdk::PluginId::new("tasks").unwrap(),
                    version: "1.2.3".parse().unwrap(),
                    name: name.clone(),
                    enabled: true,
                    switched_on: true,
                    compatible: true,
                    forced: false,
                    notices: notice
                        .iter()
                        .map(|text| sdk::PluginNotice {
                            id: "n1".into(),
                            kind: "storeReset".into(),
                            text: text.clone(),
                            at: "2026-01-01T00:00:00.000Z".into(),
                            dismissible: false,
                        })
                        .collect(),
                    reason,
                    notice,
                    build: "1.2.3-0123456789ab".into(),
                    entry: Some("/plugins/k/tasks/1.2.3-0123456789ab/dist/index.js".into()),
                    aliases: vec!["/tasks".into()],
                    permissions: vec![sdk::Permission::Storage],
                };
                same(&status, &PluginStatus::from(status.clone()));
            }
        }
        let doc = sdk::Doc {
            name: "events/2026-10".into(),
            revision: 4,
            updated_at: "2026-10-17T00:00:00.000Z".into(),
            value: json!({"schemaVersion": 1, "events": []}),
        };
        same(&doc, &PluginDoc::from(doc.clone()));
        let meta = sdk::DocMeta {
            name: doc.name.clone(),
            revision: 4,
            updated_at: doc.updated_at.clone(),
        };
        same(&meta, &PluginDocMeta::from(meta.clone()));
        for updated_by in [None, Some("ada".to_string())] {
            let version = sdk::DocVersion {
                revision: 3,
                updated_at: doc.updated_at.clone(),
                updated_by,
                value: json!(null),
            };
            same(&version, &PluginDocVersion::from(version.clone()));
        }
        for current in [None, Some(doc.clone())] {
            let conflict = sdk::Conflict {
                error: sdk::codes::CONFLICT.into(),
                message: "moved".into(),
                doc: "state".into(),
                current: current.clone(),
            };
            let mirror = PluginDocConflict {
                error: conflict.error.clone(),
                message: conflict.message.clone(),
                doc: conflict.doc.clone(),
                current: current.map(PluginCurrentDoc::from).into(),
            };
            same(&conflict, &mirror);
        }
    }

    #[test]
    fn request_bodies_read_what_the_sdk_writes() {
        for expected in [None, Some(7)] {
            let put = sdk::PutDoc {
                expected_revision: expected,
                value: json!({"a": [1, 2]}),
            };
            let mirror: PluginPutDoc = serde_json::from_value(serde_json::to_value(&put).unwrap()).unwrap();
            assert_eq!(sdk::PutDoc::from(mirror), put);
        }
        let batch = sdk::Batch {
            writes: vec![sdk::DocWrite {
                name: "state".into(),
                expected_revision: Some(1),
                value: json!("v"),
            }],
        };
        let mirror: PluginBatch = serde_json::from_value(serde_json::to_value(&batch).unwrap()).unwrap();
        assert_eq!(mirror.writes.into_iter().map(sdk::DocWrite::from).collect::<Vec<_>>(), batch.writes);
        assert!(
            serde_json::from_value::<PluginPutDoc>(json!({"expectedRevision": null})).is_err(),
            "a write without a value is refused"
        );
        let null_value: PluginPutDoc = serde_json::from_value(json!({"expectedRevision": 2, "value": null})).unwrap();
        assert_eq!(null_value.value, Value::Null, "null is a value like any other");
    }
}
