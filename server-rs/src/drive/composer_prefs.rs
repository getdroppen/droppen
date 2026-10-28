//! Composer model / effort / permission choices, shared across devices.
//!
//! Port of the former TypeScript server's `composer-prefs.ts`. A choice is
//! merged into the source's default (so new sessions inherit the latest pick)
//! and, when a session is named, into that session's entry too. The session
//! bucket is capped at [`MAX_SESSION_ENTRIES`], oldest `updatedAt` first.
//!
//! The values are bounded by the same regexes the send routes accept, because
//! they are handed to a CLI as flag values later: a stored `--dangerously-…`
//! would otherwise read as a flag rather than as a model name.
//!
//! # Where it lives
//!
//! In the machine-wide state database ([`crate::db`]) under [`scope::COMPOSER`],
//! split the way the document was: [`DEFAULTS_KEY`] holds the per-source
//! defaults as one row, and every session gets a row of its own keyed
//! `"<source>/<sessionId>"`. Session rows are separate because there are
//! hundreds of them and each pick touches exactly one — picking a model in two
//! sessions at once can no longer cost one of the picks, which is what rewriting
//! the whole document did. The defaults stay together because there are two of
//! them, claude and codex, and they are the same row the same pick writes.
//!
//! `composer-prefs.json` used to be written in place, so a crash mid-write cost
//! every remembered pick on the machine at once. It was a mirror for one
//! version, so a `.prev` binary that had never heard of this database would
//! still find the picks; the rollback floor has since risen to a build that
//! reads the database. Nothing reads the file any more.

use std::collections::BTreeMap;
use std::sync::Arc;

use gotcha::Schematic;
use serde::{Deserialize, Serialize};

use crate::db::{scope, Db};
use crate::openapi_ext::Optional;
use crate::sessions::reference::local_key;
use crate::sessions::{iso8601, Paths, Source};

/// How many per-session entries survive; the oldest beyond it are dropped.
///
/// Not a cap the *writes* need any more: a pick is one upserted row, so the
/// table would not care about the three-hundred-and-first session. The reads
/// need it. [`ComposerPrefsStore::read`] assembles the whole scope on every
/// `GET /api/composer-prefs`, and a pick for a session nobody has opened in a
/// year is not worth carrying through that for the life of the install.
pub const MAX_SESSION_ENTRIES: usize = 300;

/// The row the per-source defaults live in.
///
/// It cannot collide with a session's row: every one of those is
/// `format!("{source}/{session_id}")` and therefore contains a `/`, and this
/// does not.
const DEFAULTS_KEY: &str = "defaults";

/// What a model id may look like, here and on the send routes.
///
/// A word-ish name, optionally followed by one bracketed variant tag — the
/// shape of `claude-opus-4-8[1m]`, which is how the CLI is asked for the
/// long-context variant of a model that has one. The tag is spelled out rather
/// than folded into the character class on purpose: the value is handed to a
/// CLI as a flag value, and a class permissive enough to hold a bracket
/// anywhere would also hold an unbalanced one. Alphanumerics only inside it, so
/// nothing a shell would read as special can ever get in even if a caller one
/// day builds a command line out of this instead of an argv.
///
/// The name itself is `[[:word:]]`, `.`, `:` and `-`. That POSIX class is the
/// ASCII `\w` the original pattern was reaching for by writing `(?-u:\w)`
/// *inside* a character class — where it is not a flag group at all: the `?-u`
/// in the middle of it reads as a **range**, `?`(0x3F) through `u`(0x75), and
/// that range quietly admitted `\`, `` ` ``, `^`, `@`, `(`, `)` and both
/// brackets. So brackets were already getting through, by accident and
/// unbalanced; this says what is meant instead.
pub const MODEL_PATTERN: &str = r"^[A-Za-z0-9][[:word:].:-]*(?:\[[A-Za-z0-9]+\])?$";

/// A model id as an ACP agent spells it: `provider:model`, where the model
/// half is often a path (`openrouter:vendor/name`). Still nothing that could
/// read as a flag or hold a space; the agent checks it against its own list.
pub const ACP_MODEL_PATTERN: &str = r"^[A-Za-z0-9][[:word:].:/@+-]*$";

/// The pattern and length a model id for `source` is held to.
pub fn model_rule(source: Source) -> (&'static str, usize) {
    if source == Source::Acp {
        (ACP_MODEL_PATTERN, 200)
    } else {
        (MODEL_PATTERN, 100)
    }
}

/// One remembered set of composer choices.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ComposerChoice {
    /// Model id, as the CLI's `--model` takes it.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub model: Optional<String>,
    /// Reasoning effort.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub effort: Optional<String>,
    /// `--permission-mode` (claude) or `--sandbox` (codex) value.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub permission: Optional<String>,
    /// When this entry was last written, ISO-8601.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub updated_at: Optional<String>,
}

/// Every remembered choice.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Schematic)]
pub struct ComposerPrefs {
    /// Per-source fallback used for new sessions and sessions without an entry.
    pub defaults: std::collections::HashMap<String, ComposerChoice>,
    /// Keyed `source/sessionId`; oldest entries beyond the cap are dropped.
    pub sessions: std::collections::HashMap<String, ComposerChoice>,
}

/// A value that would be unsafe or meaningless to hand a CLI.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct ComposerPrefsError(pub String);

fn matches(pattern: &'static str, value: &str) -> bool {
    // The patterns are compile-time constants from the former TypeScript
    // server's `schemas.ts`; building them per call is cheap next to the commit
    // that follows.
    regex::Regex::new(pattern).expect("valid composer pattern").is_match(value)
}

/// The bounds `ComposerChoiceSchema` puts on each field.
fn validate(source: Source, choice: &ComposerChoice) -> Result<(), ComposerPrefsError> {
    let (model_pattern, model_length) = model_rule(source);
    let checks: [(&str, Option<&str>, &'static str, usize); 3] = [
        ("model", choice.model.as_deref(), model_pattern, model_length),
        ("effort", choice.effort.as_deref(), r"^[a-z]+$", 20),
        ("permission", choice.permission.as_deref(), r"^[A-Za-z][A-Za-z-]*$", 40),
    ];
    for (field, value, pattern, max_length) in checks {
        let Some(value) = value else { continue };
        if value.chars().count() > max_length || !matches(pattern, value) {
            return Err(ComposerPrefsError(format!("invalid {field}")));
        }
    }
    Ok(())
}

/// The composer preferences, over the machine-wide state database.
pub struct ComposerPrefsStore {
    paths: Arc<Paths>,
    /// All mutations funnel through this so concurrent PUTs cannot interleave
    /// their read-modify-write cycles.
    ///
    /// Still earning its keep after the JSON mirror went: [`Self::update`]
    /// reads the current entry, merges the patch over it and writes the result
    /// back, and two of those interleaved would drop the earlier of the two
    /// picks. Only the stores whose write is a single row lost their lock.
    write_lock: tokio::sync::Mutex<()>,
}

/// The document shape, which is how the rows are read and written: the two
/// buckets, sorted, so a snapshot of the store is deterministic.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredPrefs {
    defaults: BTreeMap<String, ComposerChoice>,
    sessions: BTreeMap<String, ComposerChoice>,
}

impl StoredPrefs {
    fn to_public(&self) -> ComposerPrefs {
        ComposerPrefs {
            defaults: self.defaults.clone().into_iter().collect(),
            sessions: self.sessions.clone().into_iter().collect(),
        }
    }
}

/// One choice out of arbitrary JSON, keeping only string-valued fields.
///
/// The single tolerant reader this module has: every row is parsed through it,
/// and it is the reason a hand-edited value costs its own field rather than the
/// store.
fn choice_from_value(raw: &serde_json::Value) -> Option<ComposerChoice> {
    let value = raw.as_object()?;
    let string = |name: &str| value.get(name).and_then(|v| v.as_str()).map(str::to_owned);
    Some(ComposerChoice {
        model: Optional(string("model")),
        effort: Optional(string("effort")),
        permission: Optional(string("permission")),
        updated_at: Optional(string("updatedAt")),
    })
}

/// A whole bucket of choices — the defaults row, as it was one of the file's
/// two maps.
fn choices_from_value(raw: &serde_json::Value) -> BTreeMap<String, ComposerChoice> {
    let Some(entries) = raw.as_object() else {
        return BTreeMap::new();
    };
    entries
        .iter()
        .filter_map(|(key, value)| Some((key.clone(), choice_from_value(value)?)))
        .collect()
}

/// Merge `patch` over `base`, field by field, as the TypeScript spread does.
fn merge(base: Option<&ComposerChoice>, patch: &ComposerChoice, stamp: &str) -> ComposerChoice {
    let base = base.cloned().unwrap_or_default();
    ComposerChoice {
        model: Optional(patch.model.0.clone().or(base.model.0)),
        effort: Optional(patch.effort.0.clone().or(base.effort.0)),
        permission: Optional(patch.permission.0.clone().or(base.permission.0)),
        updated_at: Optional(Some(stamp.to_string())),
    }
}

impl ComposerPrefsStore {
    /// A store rooted at the given path set.
    pub fn new(paths: Arc<Paths>) -> Self {
        Self {
            paths,
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// The machine-wide database the picks live in.
    fn db(&self) -> Arc<Db> {
        crate::db::at(self.paths.config_db())
    }

    async fn read_stored(&self) -> StoredPrefs {
        let mut prefs = StoredPrefs::default();
        for (key, encoded) in self.db().kv_scope(scope::COMPOSER) {
            // A row that will not parse is skipped, not fatal — one row at a
            // time, so a hand edit costs its own pick.
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&encoded) else {
                continue;
            };
            if key == DEFAULTS_KEY {
                prefs.defaults = choices_from_value(&value);
            } else if let Some(choice) = choice_from_value(&value) {
                prefs.sessions.insert(key, choice);
            }
        }
        prefs
    }

    /// Every remembered choice, as `GET /api/composer-prefs` returns it.
    pub async fn read(&self) -> ComposerPrefs {
        self.read_stored().await.to_public()
    }

    /// Merge a choice into the source default and, when named, the session entry.
    ///
    /// The validation happens before anything is opened or written, so a value
    /// that could read as a flag leaves no trace at all — not a row, not a file,
    /// not a database where there was none.
    pub async fn update(&self, source: Source, session_id: Option<&str>, patch: &ComposerChoice) -> Result<ComposerPrefs, ComposerPrefsError> {
        validate(source, patch)?;
        let _guard = self.write_lock.lock().await;
        let db = self.db();
        let mut prefs = self.read_stored().await;
        let stamp = iso8601(std::time::SystemTime::now());

        let merged = merge(prefs.defaults.get(source.as_str()), patch, &stamp);
        prefs.defaults.insert(source.as_str().to_string(), merged);
        write_row(&db, DEFAULTS_KEY, &prefs.defaults);

        if let Some(session_id) = session_id {
            // Server-free: which model and effort a person picked for a session
            // is theirs, not one server's, and these rows are already set on
            // both servers sharing this machine's state database.
            let key = local_key(source, session_id);
            let merged = merge(prefs.sessions.get(&key), patch, &stamp);
            write_row(&db, &key, &merged);
            prefs.sessions.insert(key, merged);

            // Eviction is the one part that still touches every session, and it
            // now happens only past the cap rather than on every pick. The rows
            // it drops are named, so a session someone else is picking for at
            // the same moment is not caught up in it.
            let before: Vec<String> = prefs.sessions.keys().cloned().collect();
            evict_oldest(&mut prefs.sessions);
            for key in before {
                if !prefs.sessions.contains_key(&key) {
                    db.kv_remove(scope::COMPOSER, &key);
                }
            }
        }

        Ok(prefs.to_public())
    }
}

/// Put one JSON-encoded row in the store, saying nothing if it will not encode.
///
/// A choice that cannot be serialised is not a reason to fail a pick; it is not
/// a thing that can happen to a struct of four optional strings.
fn write_row(db: &Db, key: &str, value: &impl Serialize) {
    match serde_json::to_string(value) {
        Ok(encoded) => db.kv_set(scope::COMPOSER, key, &encoded),
        Err(error) => tracing::warn!(key, %error, "cannot encode a composer preference"),
    }
}

/// Drop the least recently written entries down to the cap.
fn evict_oldest(sessions: &mut BTreeMap<String, ComposerChoice>) {
    if sessions.len() <= MAX_SESSION_ENTRIES {
        return;
    }
    let mut keys: Vec<(String, String)> = sessions
        .iter()
        .map(|(key, choice)| (choice.updated_at.as_deref().unwrap_or("").to_string(), key.clone()))
        .collect();
    // `localeCompare` on ISO-8601 timestamps is a plain lexicographic compare;
    // the key breaks ties so eviction is deterministic.
    keys.sort();
    for (_, key) in keys.into_iter().take(sessions.len() - MAX_SESSION_ENTRIES) {
        sessions.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, ComposerPrefsStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ComposerPrefsStore::new(Arc::new(Paths::new(dir.path().to_path_buf())));
        (dir, store)
    }

    fn choice(model: Option<&str>, effort: Option<&str>) -> ComposerChoice {
        ComposerChoice {
            model: Optional(model.map(str::to_owned)),
            effort: Optional(effort.map(str::to_owned)),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_session_pick_also_becomes_the_source_default() {
        let (_dir, store) = store();
        let prefs = store
            .update(Source::Claude, Some("abc"), &choice(Some("opus"), None))
            .await
            .expect("update");
        assert_eq!(prefs.defaults["claude"].model.as_deref(), Some("opus"));
        assert_eq!(prefs.sessions["claude/abc"].model.as_deref(), Some("opus"));
        assert!(prefs.sessions["claude/abc"].updated_at.as_deref().is_some(), "every write is stamped");

        // A later partial pick merges rather than replacing.
        let prefs = store
            .update(Source::Claude, Some("abc"), &choice(None, Some("high")))
            .await
            .expect("update");
        assert_eq!(
            prefs.sessions["claude/abc"].model.as_deref(),
            Some("opus"),
            "the untouched field survives"
        );
        assert_eq!(prefs.sessions["claude/abc"].effort.as_deref(), Some("high"));
        assert!(store.read().await.sessions.contains_key("claude/abc"), "and it is on disk");
    }

    #[tokio::test]
    async fn a_default_without_a_session_touches_only_the_default_bucket() {
        let (_dir, store) = store();
        let prefs = store.update(Source::Codex, None, &choice(Some("gpt-5"), None)).await.expect("update");
        assert_eq!(prefs.defaults["codex"].model.as_deref(), Some("gpt-5"));
        assert!(prefs.sessions.is_empty());
    }

    #[tokio::test]
    async fn a_value_that_could_read_as_a_flag_is_refused() {
        let (_dir, store) = store();
        assert!(store.update(Source::Claude, None, &choice(Some("--dangerously"), None)).await.is_err());
        assert!(
            store.update(Source::Claude, None, &choice(None, Some("HIGH"))).await.is_err(),
            "effort is lowercase only"
        );
        assert!(store
            .update(
                Source::Claude,
                None,
                &ComposerChoice {
                    permission: Optional(Some("--sandbox".into())),
                    ..Default::default()
                }
            )
            .await
            .is_err());
        let prefs = store.read().await;
        assert!(
            prefs.defaults.is_empty() && prefs.sessions.is_empty(),
            "a refused patch leaves no trace: not a row, not a database where there was none"
        );
    }

    /// The stored pick is what the next send is built from, so it has to accept
    /// exactly what the send routes accept — a variant tag included, or a
    /// long-context pick would be refused on its way to disk and silently
    /// forgotten between devices.
    #[tokio::test]
    async fn a_long_context_variant_tag_survives_the_round_trip() {
        let (_dir, store) = store();
        let prefs = store
            .update(Source::Claude, Some("abc"), &choice(Some("claude-opus-4-8[1m]"), None))
            .await
            .expect("update");
        assert_eq!(prefs.sessions["claude/abc"].model.as_deref(), Some("claude-opus-4-8[1m]"));
        assert!(
            store.update(Source::Claude, None, &choice(Some("opus[1m"), None)).await.is_err(),
            "an unbalanced bracket is not a variant tag"
        );
        assert!(store.update(Source::Claude, None, &choice(Some("opus[$(id)]"), None)).await.is_err());
    }

    #[tokio::test]
    async fn a_pick_is_still_there_through_a_fresh_connection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        let store = ComposerPrefsStore::new(Arc::new(paths.clone()));
        store
            .update(Source::Claude, Some("abc"), &choice(Some("opus"), None))
            .await
            .expect("update");

        let reopened = crate::db::Db::open(paths.config_db());
        let rows = reopened.kv_scope(scope::COMPOSER);
        assert!(rows.contains_key(DEFAULTS_KEY), "the defaults are one row: {rows:?}");
        assert!(
            rows.get("claude/abc").is_some_and(|row| row.contains("opus")),
            "and the session has a row of its own: {rows:?}"
        );
    }

    #[test]
    fn eviction_drops_the_least_recently_written_entries() {
        let mut sessions: BTreeMap<String, ComposerChoice> = (0..MAX_SESSION_ENTRIES + 5)
            .map(|n| {
                (
                    format!("claude/{n:04}"),
                    ComposerChoice {
                        updated_at: Optional(Some(format!("2024-01-01T00:00:00.{n:04}Z"))),
                        ..Default::default()
                    },
                )
            })
            .collect();
        evict_oldest(&mut sessions);
        assert_eq!(sessions.len(), MAX_SESSION_ENTRIES);
        assert!(!sessions.contains_key("claude/0000"), "the oldest went first");
        assert!(
            sessions.contains_key(&format!("claude/{:04}", MAX_SESSION_ENTRIES + 4)),
            "the newest stayed"
        );
    }
}
