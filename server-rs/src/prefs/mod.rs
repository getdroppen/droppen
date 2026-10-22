//! UI preferences.
//!
//! Port of the former TypeScript server's `prefs.ts`: five settings, one row
//! each in the machine-wide state database ([`crate::db`]) under
//! [`scope::PREFS`], keyed by the same camelCase names the wire uses. A row per
//! field rather than one row holding the document, because that is the shape
//! the store already had — five independent settings that happen to be saved
//! together — and because it is what makes the read below lenient for free.
//!
//! The read is deliberately lenient — every field falls back on its own, so a
//! value this build does not recognise (a newer client, a hand-edited file)
//! costs the user that one setting instead of all five. `prefs.json` used to be
//! written straight over itself, so a crash mid-write cost them all five at
//! once; five rows in one transaction cannot land in halves.
//!
//! `prefs.json` is read exactly once, by the import on the first boot after the
//! upgrade, and never written again. It was a mirror for one version, so a
//! `.prev` binary that had never heard of this database would still find the
//! settings; the rollback floor has since risen to a build that reads the
//! database. The file is left where it is rather than deleted: it costs nothing,
//! and it is the cheapest way to seed a database that has to be rebuilt from
//! scratch.

use gotcha::{api, Json, Schematic, State};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::error::ApiError;
use crate::api::extract::ApiJson;
use crate::api::{ApiRouter, AppState};
use crate::db::scope;
use crate::openapi_ext::ExtraResponses;
use crate::sessions::Paths;

/// How the session list is grouped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum SessionGrouping {
    /// One flat list, newest first.
    #[default]
    Flat,
    /// Grouped by the session's working directory.
    Directory,
}

/// What sends a composer message.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum ComposerSend {
    /// Enter sends.
    #[default]
    Enter,
    /// Cmd/Ctrl+Enter sends; Enter breaks the line.
    Modifier,
}

/// UI scale, applied as the document root font size.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum FontSize {
    /// Smaller than the browser default.
    Small,
    /// The browser default.
    #[default]
    Default,
    /// One step up.
    Large,
    /// Two steps up.
    Xlarge,
}

/// UI language.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// Follow the browser locale.
    #[default]
    Auto,
    /// English.
    En,
    /// Simplified Chinese.
    #[serde(rename = "zh-CN")]
    ZhCn,
    /// Traditional Chinese.
    #[serde(rename = "zh-TW")]
    ZhTw,
    /// Japanese.
    Ja,
}

/// How transcripts render.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptStyle {
    /// A flat list of items.
    #[default]
    Flat,
    /// Chat bubbles.
    Bubble,
}

/// The stored UI preferences.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct Prefs {
    /// How the session list is grouped.
    pub session_grouping: SessionGrouping,
    /// What sends a composer message.
    pub composer_send: ComposerSend,
    /// UI scale.
    pub font_size: FontSize,
    /// UI language; `auto` follows the browser locale.
    pub language: Language,
    /// How transcripts render.
    pub transcript_style: TranscriptStyle,
}

/// One field of a parsed preferences file, or its default.
///
/// Going through `serde_json::from_value` keeps the recognised spellings in the
/// enum's serde attributes rather than repeating them here, so the wire values
/// and the fallback rule cannot drift apart.
fn field<T: DeserializeOwned + Default>(raw: &Value, key: &str) -> T {
    raw.get(key)
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

/// Map a parsed preferences file onto [`Prefs`], field by field.
///
/// Split out from [`read_prefs`] because this — not the I/O — is the part with
/// behaviour worth pinning down.
fn prefs_from_value(raw: &Value) -> Prefs {
    Prefs {
        session_grouping: field(raw, "sessionGrouping"),
        composer_send: field(raw, "composerSend"),
        font_size: field(raw, "fontSize"),
        language: field(raw, "language"),
        transcript_style: field(raw, "transcriptStyle"),
    }
}

/// The database, with `prefs.json` drained into it if nobody has yet.
fn store(paths: &Paths) -> std::sync::Arc<crate::db::Db> {
    let db = crate::db::at(paths.config_db());
    db.import_once(scope::PREFS, || {
        // Only string values cross: a field of the wrong JSON type would read as
        // its default anyway, and importing it would put a value in the table
        // that nothing can ever parse.
        let Value::Object(map) = parse_file(paths) else {
            return Vec::new();
        };
        map.into_iter()
            .filter_map(|(key, value)| Some((key, value.as_str()?.to_string())))
            .collect()
    });
    db
}

/// The file, parsed. Read exactly once per install, by the import.
fn parse_file(paths: &Paths) -> Value {
    std::fs::read_to_string(paths.prefs_file())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

/// The stored preferences; an empty store reads as the defaults.
///
/// The rows are reassembled into the object [`prefs_from_value`] already knows
/// how to read, rather than parsed field by field here. That keeps the fallback
/// rule and the recognised spellings in exactly one place — a setting saved by a
/// newer client and a setting hand-edited into nonsense degrade identically, and
/// they always did.
pub async fn read_prefs(paths: &Paths) -> Prefs {
    let stored = store(paths).kv_scope(scope::PREFS);
    let raw = Value::Object(stored.into_iter().map(|(key, value)| (key, Value::String(value))).collect());
    prefs_from_value(&raw)
}

/// Persist the preferences, in one transaction.
///
/// Still `io::Result` because [`put_prefs`] turns its error into the route's
/// `500` and that path is part of the contract. Only the encode of a
/// five-field struct of enums can fail now, which is to say nothing can; the
/// commit itself logs rather than returns, as every other store's does.
pub async fn write_prefs(paths: &Paths, prefs: &Prefs) -> std::io::Result<()> {
    let encoded = serde_json::to_value(prefs).map_err(std::io::Error::other)?;
    let rows: Vec<(String, String)> = encoded
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
        .collect();
    // Replaced whole rather than upserted field by field, because that is what
    // this route means: the client PUTs the entire object, and a setting it
    // omitted is a setting it wants at its default.
    store(paths).kv_replace_scope(scope::PREFS, rows.iter().map(|(key, value)| (key.as_str(), value.as_str())));
    Ok(())
}

/// The stored UI preferences.
#[api(id = "get_prefs", group = "prefs", summary = "UI preferences")]
pub async fn get_prefs(State(state): State<AppState>) -> Json<Prefs> {
    Json(read_prefs(state.paths.as_ref()).await)
}

/// Persist the UI preferences and hand them straight back.
#[api(id = "put_prefs", group = "prefs", summary = "Update UI preferences")]
pub async fn put_prefs(State(state): State<AppState>, ApiJson(prefs): ApiJson<Prefs>) -> Result<Json<Prefs>, ApiError> {
    write_prefs(state.paths.as_ref(), &prefs)
        .await
        .map_err(|error| ApiError::internal(format!("failed to write prefs: {error}")))?;
    Ok(Json(prefs))
}

/// Register this subsystem's routes.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router.get("/api/prefs", get_prefs).put("/api/prefs", put_prefs)
}

/// Everything gotcha cannot express about those routes.
///
/// `PUT` re-states its `200` through [`ExtraResponses::body`] rather than
/// [`ExtraResponses::describe`]: the handler returns a `Result`, from which
/// gotcha derives a `default` response, and re-stating the success case is the
/// only way to drop it. The contract documents this route's `200` alone — a
/// failed write is a `500` there too, just an undocumented one.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe("GET", "/api/prefs", 200, "UI preferences")
        .body::<Prefs>("PUT", "/api/prefs", 200, "Updated preferences");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_reads_as_the_defaults() {
        let prefs = prefs_from_value(&Value::Null);
        assert_eq!(prefs, Prefs::default());
        assert_eq!(prefs.session_grouping, SessionGrouping::Flat);
        assert_eq!(prefs.composer_send, ComposerSend::Enter);
        assert_eq!(prefs.font_size, FontSize::Default);
        assert_eq!(prefs.language, Language::Auto);
        assert_eq!(prefs.transcript_style, TranscriptStyle::Flat);
    }

    #[test]
    fn every_field_is_read_when_it_is_recognised() {
        let raw = serde_json::json!({
            "sessionGrouping": "directory",
            "composerSend": "modifier",
            "fontSize": "xlarge",
            "language": "zh-TW",
            "transcriptStyle": "bubble",
        });
        let prefs = prefs_from_value(&raw);
        assert_eq!(prefs.session_grouping, SessionGrouping::Directory);
        assert_eq!(prefs.composer_send, ComposerSend::Modifier);
        assert_eq!(prefs.font_size, FontSize::Xlarge);
        assert_eq!(prefs.language, Language::ZhTw);
        assert_eq!(prefs.transcript_style, TranscriptStyle::Bubble);
    }

    #[test]
    fn an_unrecognised_value_only_costs_its_own_field() {
        let raw = serde_json::json!({
            "sessionGrouping": "directory",
            "fontSize": "gigantic",
            "language": 42,
            "transcriptStyle": "bubble",
        });
        let prefs = prefs_from_value(&raw);
        assert_eq!(prefs.font_size, FontSize::Default, "an unknown size falls back on its own");
        assert_eq!(prefs.language, Language::Auto, "so does a value of the wrong JSON type");
        assert_eq!(prefs.composer_send, ComposerSend::Enter, "an absent field is a default");
        assert_eq!(prefs.session_grouping, SessionGrouping::Directory, "the readable fields survive");
        assert_eq!(prefs.transcript_style, TranscriptStyle::Bubble);
    }

    #[test]
    fn a_file_that_is_not_an_object_reads_as_the_defaults() {
        for raw in [serde_json::json!("nonsense"), serde_json::json!([1, 2]), serde_json::json!(7)] {
            assert_eq!(prefs_from_value(&raw), Prefs::default());
        }
    }

    #[test]
    fn the_wire_spellings_are_the_contract_ones() {
        let prefs = Prefs {
            session_grouping: SessionGrouping::Directory,
            composer_send: ComposerSend::Modifier,
            font_size: FontSize::Xlarge,
            language: Language::ZhCn,
            transcript_style: TranscriptStyle::Bubble,
        };
        let encoded = serde_json::to_value(&prefs).expect("encode");
        assert_eq!(encoded["sessionGrouping"], "directory");
        assert_eq!(encoded["composerSend"], "modifier");
        assert_eq!(encoded["fontSize"], "xlarge");
        assert_eq!(encoded["language"], "zh-CN", "the locale tags are literal, not kebab-cased");
        assert_eq!(encoded["transcriptStyle"], "bubble");
    }

    fn paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        (dir, paths)
    }

    fn saved() -> Prefs {
        Prefs {
            session_grouping: SessionGrouping::Directory,
            composer_send: ComposerSend::Modifier,
            font_size: FontSize::Large,
            language: Language::Ja,
            transcript_style: TranscriptStyle::Bubble,
        }
    }

    /// The file is an input, once, and never an output. A save that wrote it
    /// back would be a save that a hand-restored `prefs.json` could not survive
    /// being imported from.
    #[tokio::test]
    async fn a_save_round_trips_and_writes_no_file() {
        let (_dir, paths) = paths();
        assert_eq!(read_prefs(&paths).await, Prefs::default(), "an empty store is the defaults");

        write_prefs(&paths, &saved()).await.expect("write");
        assert_eq!(read_prefs(&paths).await, saved());
        assert!(!paths.prefs_file().exists(), "no file is created where there was none");
    }

    #[tokio::test]
    async fn a_pre_existing_file_is_read_and_then_left_untouched() {
        let (_dir, paths) = paths();
        std::fs::create_dir_all(paths.config_dir()).expect("config dir");
        let before = "{\n  \"language\": \"ja\"\n}\n";
        std::fs::write(paths.prefs_file(), before).expect("seed");

        assert_eq!(read_prefs(&paths).await.language, Language::Ja, "the import read it");
        write_prefs(&paths, &Prefs::default()).await.expect("write");
        assert_eq!(read_prefs(&paths).await.language, Language::Auto, "and the save took");
        assert_eq!(
            std::fs::read_to_string(paths.prefs_file()).expect("read"),
            before,
            "byte for byte as it was found, import and save notwithstanding"
        );
    }

    /// The file is drained on the first look and never consulted again, so a
    /// setting the user has since changed is not undone by the file that still
    /// remembers the old one.
    #[tokio::test]
    async fn the_file_is_imported_once_and_then_stops_being_the_truth() {
        let (_dir, paths) = paths();
        std::fs::create_dir_all(paths.config_dir()).expect("config dir");
        std::fs::write(paths.prefs_file(), r#"{"language":"ja","fontSize":42}"#).expect("seed");

        let prefs = read_prefs(&paths).await;
        assert_eq!(prefs.language, Language::Ja, "what was in the file is in the store");
        assert_eq!(
            prefs.font_size,
            FontSize::Default,
            "and a field of the wrong type still costs only itself"
        );

        std::fs::write(paths.prefs_file(), r#"{"language":"en"}"#).expect("re-seed");
        assert_eq!(read_prefs(&paths).await.language, Language::Ja, "the import does not run a second time");
    }

    #[tokio::test]
    async fn a_setting_is_still_there_through_a_fresh_connection() {
        let (_dir, paths) = paths();
        write_prefs(&paths, &saved()).await.expect("write");

        let reopened = crate::db::Db::open(paths.config_db());
        assert_eq!(reopened.kv_scope(scope::PREFS).get("language").map(String::as_str), Some("ja"));
        assert_eq!(
            reopened.kv_scope(scope::PREFS).len(),
            5,
            "one row per field, and the row is the wire spelling"
        );
    }
}
