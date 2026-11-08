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
//! In the machine-wide state database ([`droppen_instance::db`]) under [`scope::COMPOSER`],
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

use droppen_instance::db::{scope, Db};
use droppen_http::openapi_ext::Optional;
use droppen_instance::paths::Paths;
use droppen_sessions::reference::local_key;
use droppen_provider::models::ModelRule;
use droppen_sessions::Source;
use droppen_base::time::iso8601;

/// How many per-session entries survive; the oldest beyond it are dropped.
///
/// Not a cap the *writes* need any more: a pick is one upserted row, so the
/// table would not care about the three-hundred-and-first session. The reads
/// need it. [`ComposerPrefsStore::read`] assembles the whole scope on every
/// `GET /api/composer-prefs`, and a pick for a session nobody has opened in a
/// year is not worth carrying through that for the life of the install.
pub const MAX_SESSION_ENTRIES: usize = 300;

/// Modes that drop a CLI's guard rails.
///
/// Kept in step with `DANGER_PERMISSIONS` in
/// `web/src/features/sessions/composer-controls.tsx`: the composer draws these
/// in the danger colour and will not adopt one for a session that did not pick
/// it, and a message must not quietly make one a session's stored choice.
/// `danger-full-access` is codex's alias for `full-access` in `codex exec`.
const DANGEROUS_PERMISSIONS: [&str; 4] = ["bypassPermissions", "full-access", "danger-full-access", "bypass-all"];

/// Whether `permission` is one of [`DANGEROUS_PERMISSIONS`].
fn is_dangerous(permission: &str) -> bool {
    DANGEROUS_PERMISSIONS.contains(&permission)
}

/// The row the per-source defaults live in.
///
/// It cannot collide with a session's row: every one of those is
/// `format!("{source}/{session_id}")` and therefore contains a `/`, and this
/// does not.
const DEFAULTS_KEY: &str = "defaults";

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

/// The bounds `ComposerChoiceSchema` puts on each field: the send routes' own
/// ([`crate::routes::validate_flags`]), so a choice the composer keeps
/// is one a message can be sent with.
fn validate(rule: ModelRule, choice: &ComposerChoice) -> Result<(), ComposerPrefsError> {
    crate::routes::validate_flags(rule, choice.model.as_deref(), choice.effort.as_deref(), choice.permission.as_deref()).map_err(ComposerPrefsError)
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
        droppen_instance::db::at(self.paths.config_db())
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

    /// One session's remembered choice, read as its one row rather than with
    /// every other: for a reader that asks on every look at a session.
    pub fn session(&self, source: Source, session_id: &str) -> Option<ComposerChoice> {
        let encoded = self.db().kv_get(scope::COMPOSER, &local_key(source, session_id))?;
        choice_from_value(&serde_json::from_str(&encoded).ok()?)
    }

    /// Merge a choice into the source default and, when named, the session entry.
    ///
    /// The validation happens before anything is opened or written, so a value
    /// that could read as a flag leaves no trace at all — not a row, not a file,
    /// not a database where there was none.
    pub async fn update(&self, source: Source, rule: ModelRule, session_id: Option<&str>, patch: &ComposerChoice) -> Result<ComposerPrefs, ComposerPrefsError> {
        validate(rule, patch)?;
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
            evict(&db, &mut prefs.sessions);
        }

        Ok(prefs.to_public())
    }

    /// Fill a session's missing permission from a message that was just sent.
    ///
    /// This is not a pick: the request may carry a value inherited from an
    /// older client's source default or from a stale cache, so it must not be
    /// able to change a session's stored choice. A row that already names a
    /// permission — an explicit pick or the mode the session started with — is
    /// left alone, and a dangerous mode is never stored this way at all. Only
    /// [`Self::update`] (an explicit pick in that session) may change a
    /// permission or store a dangerous one; [`Self::record_start_permission`]
    /// stores the explicit choice a new session was started with.
    ///
    /// The default row is deliberately untouched — this records a run, it is
    /// not a pick, and a message must not change what the next brand-new
    /// session starts from. A row created here has no earlier pick of its own,
    /// so it is stamped with the moment it first appears.
    ///
    /// The one row is read and written on its own rather than by scanning the
    /// scope: this runs on the message-send path. Only a row's *first* write
    /// can grow the bucket, so the cap is applied only then.
    pub async fn pin_permission(&self, source: Source, rule: ModelRule, session_id: &str, permission: &str) -> Result<(), ComposerPrefsError> {
        let patch = ComposerChoice {
            permission: Optional(Some(permission.to_string())),
            ..Default::default()
        };
        validate(rule, &patch)?;
        if is_dangerous(permission) {
            // A dangerous mode this session did not pick may be the very
            // escalation #563 caused; it never becomes the session's choice.
            return Ok(());
        }
        let _guard = self.write_lock.lock().await;
        let existing = self.session(source, session_id);
        if existing.as_ref().and_then(|choice| choice.permission.0.as_deref()).is_some() {
            return Ok(());
        }
        let created = existing.is_none();
        self.write_permission(&local_key(source, session_id), existing, &patch);
        if created {
            self.evict_sessions().await;
        }
        Ok(())
    }

    /// Record the permission a new session was explicitly started with.
    ///
    /// The new-session screen shows this choice, danger styling included,
    /// before anything is sent, and a scheduled task's mode is its own
    /// configuration. It is therefore an explicit choice — unlike a value a
    /// later message may carry — and a dangerous one is stored as such. Only
    /// called for a session this server just created, so there is no earlier
    /// pick to preserve; if a row already exists (a replay of the same start)
    /// its other fields and `updatedAt` are kept.
    pub async fn record_start_permission(&self, source: Source, rule: ModelRule, session_id: &str, permission: &str) -> Result<(), ComposerPrefsError> {
        let patch = ComposerChoice {
            permission: Optional(Some(permission.to_string())),
            ..Default::default()
        };
        validate(rule, &patch)?;
        let _guard = self.write_lock.lock().await;
        let existing = self.session(source, session_id);
        let created = existing.is_none();
        self.write_permission(&local_key(source, session_id), existing, &patch);
        if created {
            self.evict_sessions().await;
        }
        Ok(())
    }

    /// Trim the session bucket to [`MAX_SESSION_ENTRIES`] after a row was added.
    ///
    /// Every active session now gets a row from its first send or start, not
    /// only from a pick, so without this the bucket grew for the life of the
    /// install. Deciding which rows are oldest needs the bucket; that is one
    /// scope read per session created, not per message.
    async fn evict_sessions(&self) {
        let db = self.db();
        let mut prefs = self.read_stored().await;
        evict(&db, &mut prefs.sessions);
    }

    /// Merge one permission into one row, keeping its pick time and other fields.
    fn write_permission(&self, key: &str, existing: Option<ComposerChoice>, patch: &ComposerChoice) {
        let stamp = existing
            .as_ref()
            .and_then(|choice| choice.updated_at.0.clone())
            .unwrap_or_else(|| iso8601(std::time::SystemTime::now()));
        let merged = merge(existing.as_ref(), patch, &stamp);
        write_row(&self.db(), key, &merged);
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

/// Drop the least recently written entries down to the cap, and their rows.
fn evict(db: &Db, sessions: &mut BTreeMap<String, ComposerChoice>) {
    let before: Vec<String> = sessions.keys().cloned().collect();
    evict_oldest(sessions);
    for key in before {
        if !sessions.contains_key(&key) {
            db.kv_remove(scope::COMPOSER, &key);
        }
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
            .update(Source::Claude, ModelRule::NAMED, Some("abc"), &choice(Some("opus"), None))
            .await
            .expect("update");
        assert_eq!(prefs.defaults["claude"].model.as_deref(), Some("opus"));
        assert_eq!(prefs.sessions["claude/abc"].model.as_deref(), Some("opus"));
        assert!(prefs.sessions["claude/abc"].updated_at.as_deref().is_some(), "every write is stamped");

        // A later partial pick merges rather than replacing.
        let prefs = store
            .update(Source::Claude, ModelRule::NAMED, Some("abc"), &choice(None, Some("high")))
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
        let prefs = store.update(Source::Codex, ModelRule::NAMED, None, &choice(Some("gpt-5"), None)).await.expect("update");
        assert_eq!(prefs.defaults["codex"].model.as_deref(), Some("gpt-5"));
        assert!(prefs.sessions.is_empty());
    }

    #[tokio::test]
    async fn a_value_that_could_read_as_a_flag_is_refused() {
        let (_dir, store) = store();
        assert!(store.update(Source::Claude, ModelRule::NAMED, None, &choice(Some("--dangerously"), None)).await.is_err());
        assert!(
            store.update(Source::Claude, ModelRule::NAMED, None, &choice(None, Some("HIGH"))).await.is_err(),
            "effort is lowercase only"
        );
        assert!(store
            .update(
                Source::Claude,
                ModelRule::NAMED,
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
            .update(Source::Claude, ModelRule::NAMED, Some("abc"), &choice(Some("claude-opus-4-8[1m]"), None))
            .await
            .expect("update");
        assert_eq!(prefs.sessions["claude/abc"].model.as_deref(), Some("claude-opus-4-8[1m]"));
        assert!(
            store.update(Source::Claude, ModelRule::NAMED, None, &choice(Some("opus[1m"), None)).await.is_err(),
            "an unbalanced bracket is not a variant tag"
        );
        assert!(store.update(Source::Claude, ModelRule::NAMED, None, &choice(Some("opus[$(id)]"), None)).await.is_err());
    }

    /// One session's row reads as the whole document has it, and a session
    /// nobody picked for has none: the source default is not its choice.
    #[tokio::test]
    async fn one_session_s_choice_reads_as_the_document_has_it() {
        let (_dir, store) = store();
        store
            .update(Source::Claude, ModelRule::NAMED, Some("abc"), &choice(Some("deepseek-v4.1-flash[1m]"), Some("high")))
            .await
            .expect("update");
        let prefs = store.read().await;
        let row = store.session(Source::Claude, "abc").expect("the row");
        assert_eq!(row, prefs.sessions["claude/abc"]);
        assert_eq!(row.model.as_deref(), Some("deepseek-v4.1-flash[1m]"));
        assert_eq!(store.session(Source::Claude, "nobody-picked"), None, "the default is not a session's choice");
        assert_eq!(store.session(Source::Codex, "abc"), None, "nor is another CLI's session of the same id");
    }

    #[tokio::test]
    async fn a_pick_is_still_there_through_a_fresh_connection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        let store = ComposerPrefsStore::new(Arc::new(paths.clone()));
        store
            .update(Source::Claude, ModelRule::NAMED, Some("abc"), &choice(Some("opus"), None))
            .await
            .expect("update");

        let reopened = droppen_instance::db::Db::open(paths.config_db());
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

    fn permission(value: &str) -> ComposerChoice {
        ComposerChoice {
            permission: Optional(Some(value.to_string())),
            ..Default::default()
        }
    }

    /// #563: an existing permission is a pick's to change. A message carrying
    /// another value — a stale device's bypass, or what an older build
    /// inherited — replaces nothing, and a dangerous value never becomes a
    /// session's stored choice through a send.
    #[tokio::test]
    async fn a_send_never_changes_an_existing_permission_or_stores_a_dangerous_one() {
        let (_dir, store) = store();
        store
            .update(Source::Claude, ModelRule::NAMED, Some("abc"), &permission("bypassPermissions"))
            .await
            .expect("pick");
        // The user narrows it in the same session: an explicit pick may change it.
        store
            .update(Source::Claude, ModelRule::NAMED, Some("abc"), &permission("plan"))
            .await
            .expect("pick");
        // A stale device now sends the value it still has. The row stays plan.
        store
            .pin_permission(Source::Claude, ModelRule::NAMED, "abc", "bypassPermissions")
            .await
            .expect("pin");
        assert_eq!(
            store.session(Source::Claude, "abc").unwrap().permission.as_deref(),
            Some("plan"),
            "a send must not put the stale bypass back over the pick"
        );
        // Nor does a safe value from a send replace the stored choice.
        store.pin_permission(Source::Claude, ModelRule::NAMED, "abc", "acceptEdits").await.expect("pin");
        assert_eq!(store.session(Source::Claude, "abc").unwrap().permission.as_deref(), Some("plan"));
        // The default is a pick's, not a send's.
        assert_eq!(store.read().await.defaults["claude"].permission.as_deref(), Some("plan"));
    }

    /// A send fills a missing permission — that is how a session learned what
    /// it ran under starts keeping it — but a dangerous value is never stored
    /// this way.
    #[tokio::test]
    async fn a_send_fills_only_a_missing_permission_and_only_a_safe_mode() {
        let (_dir, store) = store();
        // A row that picked a model only: the send fills the permission and
        // leaves the model and effort alone.
        store
            .update(Source::Claude, ModelRule::NAMED, Some("abc"), &choice(Some("opus"), Some("high")))
            .await
            .expect("update");
        store.pin_permission(Source::Claude, ModelRule::NAMED, "abc", "read-only").await.expect("pin");
        let row = store.session(Source::Claude, "abc").expect("row");
        assert_eq!(row.permission.as_deref(), Some("read-only"));
        assert_eq!(row.model.as_deref(), Some("opus"));
        assert_eq!(row.effort.as_deref(), Some("high"));
        assert_eq!(
            store.read().await.defaults["claude"].permission.0,
            None,
            "a send writes no default"
        );

        // A dangerous value from a send never creates a row for a session that
        // had none (the version-skew and stale-cache cases), including codex's
        // `danger-full-access` alias for `full-access`.
        store
            .pin_permission(Source::Claude, ModelRule::NAMED, "other", "bypassPermissions")
            .await
            .expect("pin");
        store
            .pin_permission(Source::Codex, ModelRule::NAMED, "other", "danger-full-access")
            .await
            .expect("pin");
        assert_eq!(store.session(Source::Claude, "other"), None);
        assert_eq!(store.session(Source::Codex, "other"), None);
        assert_eq!(store.read().await.sessions.len(), 1, "no row is left behind");
    }

    /// The permission a session is *started* with is explicit — the new-session
    /// screen shows it, danger styling included — so it is recorded as such.
    #[tokio::test]
    async fn a_start_records_its_permission_dangerous_included() {
        let (_dir, store) = store();
        store
            .record_start_permission(Source::Claude, ModelRule::NAMED, "started", "bypassPermissions")
            .await
            .expect("record");
        let row = store.session(Source::Claude, "started").expect("row");
        assert_eq!(row.permission.as_deref(), Some("bypassPermissions"));
        assert!(row.updated_at.as_deref().is_some(), "a new row is stamped");
        assert!(store.read().await.defaults.is_empty(), "a start writes no default");

        store
            .record_start_permission(Source::Codex, ModelRule::NAMED, "started", "read-only")
            .await
            .expect("record");
        assert_eq!(store.session(Source::Codex, "started").unwrap().permission.as_deref(), Some("read-only"));
    }

    #[tokio::test]
    async fn a_send_fills_a_session_with_no_row() {
        let (_dir, store) = store();
        store.pin_permission(Source::Claude, ModelRule::NAMED, "abc", "plan").await.expect("pin");
        let prefs = store.read().await;
        assert_eq!(prefs.sessions["claude/abc"].permission.as_deref(), Some("plan"));
        assert!(prefs.sessions["claude/abc"].updated_at.as_deref().is_some(), "a new row is stamped");
        assert!(prefs.defaults.is_empty(), "and no default is invented from it");
    }

    /// Every active session now gets a row from its first send or start, not
    /// only from a pick, so creating rows has to respect the cap too.
    #[tokio::test]
    async fn creating_rows_through_sends_evicts_them_down_to_the_cap() {
        let (_dir, store) = store();
        for n in 0..MAX_SESSION_ENTRIES + 5 {
            let id = format!("{n:04}");
            store.pin_permission(Source::Claude, ModelRule::NAMED, &id, "plan").await.expect("pin");
        }
        let prefs = store.read().await;
        assert_eq!(prefs.sessions.len(), MAX_SESSION_ENTRIES, "the bucket is capped");
        assert!(!prefs.sessions.contains_key("claude/0000"), "the oldest row went first");
        assert!(
            prefs.sessions.contains_key(&format!("claude/{:04}", MAX_SESSION_ENTRIES + 4)),
            "the newest row stayed"
        );
    }

    #[tokio::test]
    async fn creating_rows_through_starts_evicts_them_down_to_the_cap() {
        let (_dir, store) = store();
        for n in 0..MAX_SESSION_ENTRIES + 5 {
            let id = format!("{n:04}");
            store.record_start_permission(Source::Codex, ModelRule::NAMED, &id, "read-only").await.expect("record");
        }
        let prefs = store.read().await;
        assert_eq!(prefs.sessions.len(), MAX_SESSION_ENTRIES, "the bucket is capped");
        assert!(!prefs.sessions.contains_key("codex/0000"), "the oldest row went first");
        assert!(
            prefs.sessions.contains_key(&format!("codex/{:04}", MAX_SESSION_ENTRIES + 4)),
            "the newest row stayed"
        );
    }

    /// The row's `updatedAt` is the pick time the composer orders a transcript
    /// against. Pinning records a run, not a pick, and must not make an older
    /// model pick look newer than the turn that ran with it.
    #[tokio::test]
    async fn pinning_does_not_restamp_the_row_it_writes_to() {
        let (_dir, store) = store();
        let picked = store
            .update(Source::Claude, ModelRule::NAMED, Some("abc"), &choice(Some("opus"), None))
            .await
            .expect("update");
        let picked_at = picked.sessions["claude/abc"].updated_at.0.clone();
        store.pin_permission(Source::Claude, ModelRule::NAMED, "abc", "plan").await.expect("pin");
        let prefs = store.read().await;
        assert_eq!(prefs.sessions["claude/abc"].updated_at.0, picked_at);
    }

    #[tokio::test]
    async fn a_permission_that_could_read_as_a_flag_is_refused_by_a_pin() {
        let (_dir, store) = store();
        assert!(store.pin_permission(Source::Claude, ModelRule::NAMED, "abc", "--sandbox").await.is_err());
        assert!(
            store
                .record_start_permission(Source::Claude, ModelRule::NAMED, "abc", "--sandbox")
                .await
                .is_err()
        );
        assert!(store.read().await.sessions.is_empty(), "and it leaves no row behind");
    }
}
