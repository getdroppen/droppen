//! Token usage aggregated by calendar day and model.
//!
//! Port of the former TypeScript server's `usage.ts`. Every session file that
//! was touched inside the window is folded once and cached by path+mtime, then
//! the rows are keyed on their *own* timestamps — so a session that straddles
//! the window boundary only contributes the turns that actually fall inside it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use futures_util::StreamExt;
use gotcha::Schematic;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use droppen_http::openapi_ext::Nullable;
use droppen_instance::paths::Paths;
use crate::sources::{TokenUsage, UsageFile};
use crate::util::{try_parse, LossyLines};
use crate::{Source, TranscriptSources, Transcripts};
use droppen_base::time::iso8601;

const DAY_MS: i64 = 86_400_000;

/// The default window, and the ceiling the TypeScript route enforces.
const DEFAULT_DAYS: u32 = 30;
const MAX_DAYS: u32 = 365;

/// How many session files are read at once, matching the TypeScript's `mapLimit`.
const READ_CONCURRENCY: usize = 8;

// --- wire shapes -------------------------------------------------------------

/// Window-wide totals, plus what they are estimated to have cost.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    /// Fresh input tokens.
    pub input: f64,
    /// Generated output tokens.
    pub output: f64,
    /// Tokens served from the prompt cache.
    pub cache_read: f64,
    /// Tokens written into the prompt cache (Claude only; Codex reports none).
    pub cache_write: f64,
    /// The four buckets added up.
    pub total: f64,
    /// How many session files contributed at least one row inside the window.
    pub sessions: usize,
    /// Estimated spend in USD at published rates, unpriced models excluded.
    #[serde(rename = "costUSD")]
    pub cost_usd: f64,
}

/// One calendar day of the window.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct DayUsage {
    /// Fresh input tokens.
    pub input: f64,
    /// Generated output tokens.
    pub output: f64,
    /// Tokens served from the prompt cache.
    pub cache_read: f64,
    /// Tokens written into the prompt cache.
    pub cache_write: f64,
    /// The day itself, `YYYY-MM-DD` (UTC).
    pub date: String,
    /// The four buckets added up.
    pub total: f64,
}

/// One model's share of the window, on one CLI.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    /// Fresh input tokens.
    pub input: f64,
    /// Generated output tokens.
    pub output: f64,
    /// Tokens served from the prompt cache.
    pub cache_read: f64,
    /// Tokens written into the prompt cache.
    pub cache_write: f64,
    /// Model id as the transcript recorded it.
    pub model: String,
    /// Which CLI charged for it.
    pub source: Source,
    /// The four buckets added up.
    pub total: f64,
    /// Estimated USD at published API rates, or null when the model is unpriced.
    pub cost: Nullable<f64>,
}

/// One CLI's share of the window.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SourceUsage {
    /// Which CLI.
    pub source: Source,
    /// Every bucket of every one of its models, added up.
    pub total: f64,
    /// How many of its session files contributed inside the window.
    pub sessions: usize,
    /// Estimated USD; an unpriced model contributes nothing.
    pub cost: f64,
}

/// Where the cost estimate came from, and what it could not cover.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct UsagePricing {
    /// `"{source}:{model}"` for every model with no published rate.
    pub unpriced_models: Vec<String>,
    /// The file that overrides or extends the built-in rate table.
    pub override_path: String,
}

/// Token usage aggregated by day and model over the recent window.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct UsageStats {
    /// How many UTC calendar days the window covers, ending today.
    pub window_days: u32,
    /// First day of the window, `YYYY-MM-DD` (UTC).
    pub since: String,
    /// When this snapshot was taken, ISO-8601.
    pub generated_at: String,
    /// Totals across every day, model and source.
    pub totals: UsageTotals,
    /// Every calendar day from `since` to today inclusive, oldest first, no gaps.
    pub daily: Vec<DayUsage>,
    /// Per model and source, largest total first.
    pub by_model: Vec<ModelUsage>,
    /// Per CLI, claude before codex; a source with nothing to report is dropped.
    pub by_source: Vec<SourceUsage>,
    /// How the cost estimate was arrived at.
    pub pricing: UsagePricing,
}

// --- civil dates -------------------------------------------------------------

/// Epoch milliseconds of UTC midnight on a `YYYY-MM-DD` date, JS's `Date.parse`.
fn millis_from_date(date: &str) -> Option<i64> {
    let date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis())
}

/// The UTC calendar day an instant falls on, `YYYY-MM-DD`.
fn date_from_millis(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis.max(0))
        .map(|at| at.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "1970-01-01".to_string())
}

/// `(since, cutoff)` for a window of `window_days` UTC days ending on `today`.
///
/// A window of N days is exactly N calendar days *including* today, which is why
/// only `N - 1` days are subtracted.
fn window_bounds(today: &str, window_days: u32) -> (String, i64) {
    let cutoff = millis_from_date(today).unwrap_or(0) - (i64::from(window_days) - 1) * DAY_MS;
    (date_from_millis(cutoff), cutoff)
}

/// Clamp a caller's `days` to a sane window.
fn normalize_days(days: Option<u32>) -> u32 {
    match days {
        Some(0) | None => DEFAULT_DAYS,
        Some(n) => n.min(MAX_DAYS),
    }
}

// --- folding -----------------------------------------------------------------

/// Token counts split by how the model charged for them.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Bucket {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

impl Bucket {
    fn total(self) -> f64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    fn add(&mut self, other: Bucket) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

/// One `(day, source, model)` triple's tokens, as folded out of one session file.
#[derive(Debug, Clone, PartialEq)]
struct UsageRow {
    date: String,
    source: Source,
    model: String,
    bucket: Bucket,
}

/// Rows keyed by `(date, source, model)`, kept in first-seen order.
///
/// The TypeScript folds into a `Map`, which iterates in insertion order, and
/// `byModel`'s sort is stable — so ties there fall back to first-seen. A bare
/// `HashMap` would make that order depend on hashing instead.
#[derive(Debug, Default)]
struct RowMap {
    rows: Vec<UsageRow>,
    index: HashMap<(String, Source, String), usize>,
}

impl RowMap {
    fn add(&mut self, date: &str, source: Source, model: &str, delta: Bucket) {
        let key = (date.to_string(), source, model.to_string());
        let slot = match self.index.get(&key) {
            Some(&slot) => slot,
            None => {
                let slot = self.rows.len();
                self.rows.push(UsageRow {
                    date: date.to_string(),
                    source,
                    model: model.to_string(),
                    bucket: Bucket::default(),
                });
                self.index.insert(key, slot);
                slot
            }
        };
        self.rows[slot].bucket.add(delta);
    }

    /// Add what one transcript entry said it used.
    fn add_usage(&mut self, source: Source, usage: TokenUsage) {
        let delta = Bucket {
            input: usage.input,
            output: usage.output,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
        };
        self.add(&usage.date, source, &usage.model, delta);
    }
}

// --- pricing -----------------------------------------------------------------

/// USD per one million tokens, per charge type.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ModelPrice {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

impl ModelPrice {
    /// What an override with no defaults behind it starts from.
    const ZERO: ModelPrice = ModelPrice {
        input: 0.0,
        output: 0.0,
        cache_read: 0.0,
        cache_write: 0.0,
    };
}

/// Published Anthropic pay-as-you-go rates (USD / 1M tokens).
///
/// Cache write is the 1.25x (5-minute TTL) rate and cache read is 0.1x input,
/// per Anthropic's pricing. OpenAI/Codex models are intentionally absent — the
/// user runs those on a flat subscription and we have no authoritative
/// per-token rate, so they stay unpriced rather than guessed. Override or
/// extend any of this by writing `<config>/model-prices.json` (same shape,
/// keyed by model id; `DROPPEN_CONFIG_DIR` moves the folder it lives in).
const DEFAULT_PRICES: [(&str, ModelPrice); 10] = [
    (
        "claude-fable-5",
        ModelPrice {
            input: 10.0,
            output: 50.0,
            cache_write: 12.5,
            cache_read: 1.0,
        },
    ),
    (
        "claude-mythos-5",
        ModelPrice {
            input: 10.0,
            output: 50.0,
            cache_write: 12.5,
            cache_read: 1.0,
        },
    ),
    (
        // Opus 5 is priced exactly as Opus 4.8 was — the tier's rate did not
        // move with the generation, so the row is a copy rather than a new tier.
        "claude-opus-5",
        ModelPrice {
            input: 5.0,
            output: 25.0,
            cache_write: 6.25,
            cache_read: 0.5,
        },
    ),
    (
        "claude-opus-4-8",
        ModelPrice {
            input: 5.0,
            output: 25.0,
            cache_write: 6.25,
            cache_read: 0.5,
        },
    ),
    (
        "claude-opus-4-7",
        ModelPrice {
            input: 5.0,
            output: 25.0,
            cache_write: 6.25,
            cache_read: 0.5,
        },
    ),
    (
        "claude-opus-4-6",
        ModelPrice {
            input: 5.0,
            output: 25.0,
            cache_write: 6.25,
            cache_read: 0.5,
        },
    ),
    (
        "claude-opus-4-5",
        ModelPrice {
            input: 5.0,
            output: 25.0,
            cache_write: 6.25,
            cache_read: 0.5,
        },
    ),
    (
        "claude-sonnet-5",
        ModelPrice {
            input: 3.0,
            output: 15.0,
            cache_write: 3.75,
            cache_read: 0.3,
        },
    ),
    (
        "claude-sonnet-4-6",
        ModelPrice {
            input: 3.0,
            output: 15.0,
            cache_write: 3.75,
            cache_read: 0.3,
        },
    ),
    (
        "claude-haiku-4-5",
        ModelPrice {
            input: 1.0,
            output: 5.0,
            cache_write: 1.25,
            cache_read: 0.1,
        },
    ),
];

/// Where the config folder keeps the optional per-model price table that
/// overrides the built-in rates.
///
/// Its own type rather than a method of [`Paths`]: only the usage stats read
/// it. The old `~/.remote-hand` spelling is looked at only by
/// [`PriceOverride::migrate_legacy`], which the server runs once at boot.
#[derive(Debug, Clone)]
pub struct PriceOverride {
    /// `<config>/model-prices.json`, the file every reader reads.
    path: PathBuf,
    /// `~/.remote-hand/model-prices.json`, as releases before the move wrote
    /// and read it. Kept spelled once, here, because boot is the one place
    /// that still has to know it.
    legacy: PathBuf,
}

impl PriceOverride {
    /// The override in the config folder `paths` names.
    ///
    /// In the config root rather than the user's home directly so a second
    /// install with its own `DROPPEN_CONFIG_DIR` has its own price table, and
    /// so backup advice names the same folder as everything else the server
    /// keeps.
    pub fn of(paths: &Paths) -> Self {
        Self {
            path: paths.config_dir().join("model-prices.json"),
            legacy: paths.home.join(".remote-hand").join("model-prices.json"),
        }
    }

    /// The file, which may not exist.
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }

    /// Move a `~/.remote-hand/model-prices.json` left by an earlier release
    /// into the config folder, and remove the old folder if that leaves it
    /// empty.
    ///
    /// Only `serve` calls this, only after it holds the instance lock: the
    /// reader reads [`PriceOverride::path`] and nothing else, so a command
    /// that does not serve must not move a user's file, and two instances
    /// booting together must not race the same rename (the loser finds the
    /// file gone and does nothing).
    ///
    /// If both files exist the new one wins and the old one is left alone —
    /// this cannot tell which one the user edited last, and deleting either
    /// is worse than keeping a file the reader ignores. The exception is a
    /// rename that crosses file systems: there the bytes are copied to a
    /// temporary name, fsynced and put in place, and the old file is removed
    /// only once the new one is whole.
    ///
    /// The step stays until the upgrade floor passes the release that moved
    /// the file: an install at v0.2.125 can jump straight to a later one, and
    /// its table would otherwise be ignored from then on.
    pub async fn migrate_legacy(&self) -> PriceMigration {
        // An override that points the config folder at the old folder itself
        // names the same file twice; there is nothing to move.
        if self.path == self.legacy {
            return PriceMigration::Nothing;
        }
        let Ok(metadata) = tokio::fs::metadata(&self.legacy).await else {
            return PriceMigration::Nothing;
        };
        // A directory or a dangling link is not a table a user wrote; leaving
        // it alone is the only reading that cannot destroy something.
        if !metadata.is_file() {
            return PriceMigration::Nothing;
        }
        if tokio::fs::metadata(&self.path).await.is_ok() {
            tracing::warn!(
                legacy = %self.legacy.display(),
                path = %self.path.display(),
                "both model-price tables exist; using the one in the config folder and leaving the old one where it is"
            );
            return PriceMigration::BothPresent;
        }
        if let Some(parent) = self.path.parent() {
            if let Err(error) = tokio::fs::create_dir_all(parent).await {
                tracing::error!(path = %parent.display(), %error, "cannot create the config folder for the model price table");
                return PriceMigration::Failed;
            }
        }
        match tokio::fs::rename(&self.legacy, &self.path).await {
            Ok(()) => {
                self.remove_legacy_dir().await;
                tracing::info!(
                    from = %self.legacy.display(),
                    to = %self.path.display(),
                    "moved the model price table into the config folder"
                );
                PriceMigration::Moved
            }
            Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => self.copy_across().await,
            Err(error) => {
                tracing::error!(
                    from = %self.legacy.display(),
                    to = %self.path.display(),
                    %error,
                    "cannot move the model price table; custom prices are ignored until it is moved by hand"
                );
                PriceMigration::Failed
            }
        }
    }

    /// [`PriceOverride::migrate_legacy`] when `rename` cannot cross file
    /// systems: copy, fsync, put in place, then remove the old file.
    ///
    /// The copy lands under a temporary name first. A crash halfway through a
    /// copy straight onto [`PriceOverride::path`] would leave a truncated table
    /// that the next boot's "the new file exists" rule would then prefer over
    /// the intact old one.
    async fn copy_across(&self) -> PriceMigration {
        use tokio::io::AsyncWriteExt;

        let bytes = match tokio::fs::read(&self.legacy).await {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::error!(from = %self.legacy.display(), %error, "cannot read the model price table to move it");
                return PriceMigration::Failed;
            }
        };
        let Some(parent) = self.path.parent() else {
            return PriceMigration::Failed;
        };
        let temporary = parent.join(format!(".model-prices.json.{}.migrating", std::process::id()));
        let copied = async {
            let mut file = tokio::fs::File::create(&temporary).await?;
            file.write_all(&bytes).await?;
            file.sync_all().await?;
            drop(file);
            tokio::fs::rename(&temporary, &self.path).await
        }
        .await;
        if let Err(error) = copied {
            let _ = tokio::fs::remove_file(&temporary).await;
            tracing::error!(to = %self.path.display(), %error, "cannot copy the model price table into the config folder");
            return PriceMigration::Failed;
        }
        match tokio::fs::remove_file(&self.legacy).await {
            Ok(()) => {
                self.remove_legacy_dir().await;
                tracing::info!(
                    from = %self.legacy.display(),
                    to = %self.path.display(),
                    "copied the model price table into the config folder; the old folder was on another file system"
                );
                PriceMigration::Moved
            }
            Err(error) => {
                tracing::warn!(
                    from = %self.legacy.display(),
                    %error,
                    "copied the model price table into the config folder but could not remove the old one; the new one wins"
                );
                PriceMigration::BothPresent
            }
        }
    }

    /// `~/.remote-hand` itself, when the move left it empty. A `remove_dir`
    /// fails on anything but an empty directory, which is the guard this
    /// needs: a folder holding anything else is not ours to delete.
    async fn remove_legacy_dir(&self) {
        let Some(dir) = self.legacy.parent() else { return };
        match tokio::fs::remove_dir(dir).await {
            Ok(()) => tracing::info!(path = %dir.display(), "removed the now-empty old model price folder"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            // Not empty: the user keeps something else there, or the copy
            // path's old file is still present. Silence would look like a
            // deletion that happened.
            Err(error) if error.kind() != std::io::ErrorKind::DirectoryNotEmpty => {
                tracing::warn!(path = %dir.display(), %error, "could not remove the old model price folder");
            }
            Err(_) => {}
        }
    }
}

/// What [`PriceOverride::migrate_legacy`] did, so boot can be asserted rather
/// than inferred from a directory listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceMigration {
    /// There was no `~/.remote-hand/model-prices.json`.
    Nothing,
    /// The old file is now the config folder's.
    Moved,
    /// Both files exist; the config folder's wins and the old one is left
    /// where it is.
    BothPresent,
    /// The move failed; the built-in rates are what the server prices with.
    Failed,
}

/// The built-in rates merged with the user's optional override file.
///
/// A `Vec` rather than a map because the prefix fallback walks the table in
/// order, and the TypeScript walks `Object.keys`, which is insertion order.
#[derive(Debug)]
struct PriceTable(Vec<(String, ModelPrice)>);

impl PriceTable {
    /// The built-in table alone.
    fn defaults() -> PriceTable {
        PriceTable(DEFAULT_PRICES.iter().map(|(id, price)| ((*id).to_string(), *price)).collect())
    }

    /// The built-in table with the config folder's `model-prices.json` merged
    /// over it.
    ///
    /// Re-read per request rather than memoised as the TypeScript does: the file
    /// is tiny, and a process-wide cache could not answer for the two different
    /// home directories two servers in one test process point at.
    async fn load(path: &Path) -> PriceTable {
        let mut table = PriceTable::defaults();
        let Ok(text) = tokio::fs::read_to_string(path).await else {
            return table;
        };
        let Ok(Value::Object(raw)) = serde_json::from_str::<Value>(&text) else {
            return table;
        };
        for (id, value) in raw {
            let Some(fields) = value.as_object() else { continue };
            let base = table.exact(&id).unwrap_or(ModelPrice::ZERO);
            let merged = ModelPrice {
                input: fields.get("input").and_then(Value::as_f64).unwrap_or(base.input),
                output: fields.get("output").and_then(Value::as_f64).unwrap_or(base.output),
                cache_read: fields.get("cacheRead").and_then(Value::as_f64).unwrap_or(base.cache_read),
                cache_write: fields.get("cacheWrite").and_then(Value::as_f64).unwrap_or(base.cache_write),
            };
            match table.0.iter_mut().find(|(key, _)| *key == id) {
                Some(slot) => slot.1 = merged,
                None => table.0.push((id, merged)),
            }
        }
        table
    }

    fn exact(&self, model: &str) -> Option<ModelPrice> {
        self.0.iter().find(|(id, _)| id == model).map(|(_, price)| *price)
    }

    /// The rate for a model, falling back to a prefix match so dated ids
    /// (`claude-haiku-4-5-20251001`) still price.
    fn get(&self, model: &str) -> Option<ModelPrice> {
        self.exact(model)
            .or_else(|| self.0.iter().find(|(id, _)| model.starts_with(id.as_str())).map(|(_, price)| *price))
    }
}

fn cost_of(bucket: Bucket, price: ModelPrice) -> f64 {
    (bucket.input * price.input + bucket.output * price.output + bucket.cache_read * price.cache_read + bucket.cache_write * price.cache_write)
        / 1_000_000.0
}

// --- file scanning -----------------------------------------------------------

/// One file's folded rows, keyed by the mtime they were folded from.
type CachedRows = HashMap<PathBuf, (f64, Vec<UsageRow>)>;

/// Parsed rows per file+mtime, so a refresh only re-reads sessions that changed.
static FILE_CACHE: LazyLock<Mutex<CachedRows>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Stream a single session file, holding at most one line in memory at a time.
async fn parse_file(path: &Path, reader: &dyn Transcripts) -> std::io::Result<Vec<UsageRow>> {
    let mut lines = LossyLines::open(path).await?;
    let mut rows = RowMap::default();
    let mut fold = reader.usage_fold();
    // A read error stops the walk rather than discarding the rows already
    // folded: a half-written tail line is normal while a CLI is mid-write.
    // Malformed *bytes* are not a read error — `LossyLines` substitutes the
    // replacement character and carries on — so one corrupt line can no longer
    // silently truncate a month of totals.
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let Some(entry) = try_parse(&line) else { continue };
        if let Some(usage) = fold.fold(&entry) {
            rows.add_usage(reader.source(), usage);
        }
    }
    Ok(rows.rows)
}

async fn file_rows(file: UsageFile, reader: &dyn Transcripts) -> Vec<UsageRow> {
    if let Some((mtime, rows)) = FILE_CACHE.lock().expect("usage cache poisoned").get(&file.path) {
        if *mtime == file.mtime {
            return rows.clone();
        }
    }
    // An unreadable file contributes nothing and is not cached, so a transient
    // failure does not stick until the file changes again.
    let Ok(rows) = parse_file(&file.path, reader).await else {
        return Vec::new();
    };
    FILE_CACHE
        .lock()
        .expect("usage cache poisoned")
        .insert(file.path.clone(), (file.mtime, rows.clone()));
    rows
}

async fn rows_per_file(refs: Vec<UsageFile>, reader: &dyn Transcripts) -> Vec<Vec<UsageRow>> {
    futures_util::stream::iter(refs)
        .map(|file| file_rows(file, reader))
        .buffered(READ_CONCURRENCY)
        .collect()
        .await
}

// --- aggregation -------------------------------------------------------------

/// Everything the two source passes fold into.
#[derive(Debug, Default)]
struct Accumulator {
    totals: Bucket,
    days: HashMap<String, Bucket>,
    models: Vec<ModelUsage>,
    model_index: HashMap<(Source, String), usize>,
}

impl Accumulator {
    /// Fold one source's per-file rows in, returning how many files contributed.
    fn consume(&mut self, per_file: &[Vec<UsageRow>], since: &str) -> usize {
        let mut sessions = 0;
        for rows in per_file {
            let in_window = rows.iter().filter(|row| row.date.as_str() >= since);
            let mut contributed = false;
            for row in in_window {
                contributed = true;
                self.totals.add(row.bucket);
                self.days.entry(row.date.clone()).or_default().add(row.bucket);

                let key = (row.source, row.model.clone());
                let slot = match self.model_index.get(&key) {
                    Some(&slot) => slot,
                    None => {
                        let slot = self.models.len();
                        self.models.push(ModelUsage {
                            input: 0.0,
                            output: 0.0,
                            cache_read: 0.0,
                            cache_write: 0.0,
                            model: row.model.clone(),
                            source: row.source,
                            total: 0.0,
                            cost: Nullable(None),
                        });
                        self.model_index.insert(key, slot);
                        slot
                    }
                };
                let model = &mut self.models[slot];
                model.input += row.bucket.input;
                model.output += row.bucket.output;
                model.cache_read += row.bucket.cache_read;
                model.cache_write += row.bucket.cache_write;
                model.total = model.input + model.output + model.cache_read + model.cache_write;
            }
            if contributed {
                sessions += 1;
            }
        }
        sessions
    }
}

impl TranscriptSources {
    /// Aggregate token usage across every Claude and Codex session touched within
    /// the last `days` days, grouped by calendar day and by model.
    pub async fn collect_usage(&self, paths: &Paths, days: Option<u32>) -> UsageStats {
        let window_days = normalize_days(days);
        let generated_at = iso8601(SystemTime::now());
        let today = generated_at[..10].to_string();
        let (since, cutoff_ms) = window_bounds(&today, window_days);
        let cutoff_ms = cutoff_ms as f64;

        let readers: Vec<&dyn Transcripts> = self.iter().collect();
        let refs = futures_util::future::join_all(readers.iter().map(|reader| reader.usage_files(paths, cutoff_ms))).await;
        let per_file = futures_util::future::join_all(readers.iter().zip(refs).map(|(reader, refs)| rows_per_file(refs, *reader))).await;

        // One source after the other, in `Source`'s order: a model's place in
        // `byModel` among equal totals is where it was first seen.
        let mut acc = Accumulator::default();
        let sessions: Vec<(Source, usize)> = readers
            .iter()
            .zip(&per_file)
            .map(|(reader, per_file)| (reader.source(), acc.consume(per_file, &since)))
            .collect();

        // Fill every calendar day in the window so the timeline chart has no gaps.
        let mut daily = Vec::new();
        let mut cursor = millis_from_date(&since).unwrap_or(0);
        for _ in 0..=window_days + 1 {
            let date = date_from_millis(cursor);
            let bucket = acc.days.get(&date).copied().unwrap_or_default();
            let reached_today = date >= today;
            daily.push(DayUsage {
                input: bucket.input,
                output: bucket.output,
                cache_read: bucket.cache_read,
                cache_write: bucket.cache_write,
                date,
                total: bucket.total(),
            });
            if reached_today {
                break;
            }
            cursor += DAY_MS;
        }

        // Price each model's tokens at published API rates; unpriced models (Codex/
        // OpenAI, or anything without a table entry) contribute nothing and are named.
        let prices = PriceTable::load(&PriceOverride::of(paths).path()).await;
        let mut unpriced_models = Vec::new();
        let mut cost_usd = 0.0;
        let mut by_model = acc.models;
        for model in &mut by_model {
            let bucket = Bucket {
                input: model.input,
                output: model.output,
                cache_read: model.cache_read,
                cache_write: model.cache_write,
            };
            match prices.get(&model.model) {
                Some(price) => {
                    let cost = cost_of(bucket, price);
                    model.cost = Nullable(Some(cost));
                    cost_usd += cost;
                }
                None => {
                    model.cost = Nullable(None);
                    unpriced_models.push(format!("{}:{}", model.source, model.model));
                }
            }
        }
        // Stable, so equal totals keep the order they were first seen in.
        by_model.sort_by(|a, b| b.total.partial_cmp(&a.total).unwrap_or(std::cmp::Ordering::Equal));

        let by_source = sessions
            .iter()
            .map(|&(source, sessions)| {
                let models = by_model.iter().filter(|model| model.source == source);
                let (total, cost) = models.fold((0.0, 0.0), |(total, cost), model| {
                    (total + model.total, cost + model.cost.as_ref().copied().unwrap_or(0.0))
                });
                SourceUsage {
                    source,
                    total,
                    sessions,
                    cost,
                }
            })
            .filter(|entry| entry.sessions > 0 || entry.total > 0.0)
            .collect();

        UsageStats {
            window_days,
            since,
            generated_at,
            totals: UsageTotals {
                input: acc.totals.input,
                output: acc.totals.output,
                cache_read: acc.totals.cache_read,
                cache_write: acc.totals.cache_write,
                total: acc.totals.total(),
                sessions: sessions.iter().map(|(_, sessions)| sessions).sum(),
                cost_usd,
            },
            daily,
            by_model,
            by_source,
            pricing: UsagePricing {
                unpriced_models,
                override_path: PriceOverride::of(&paths).path().to_string_lossy().into_owned(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::test_reader::TestReader;
    use serde_json::json;

    /// Fold a literal transcript the way `parse_file` folds a real one.
    fn rows_of(reader: &dyn Transcripts, entries: Value) -> Vec<UsageRow> {
        let mut rows = RowMap::default();
        let mut fold = reader.usage_fold();
        for entry in entries.as_array().expect("array") {
            if let Some(usage) = fold.fold(entry.as_object().expect("object")) {
                rows.add_usage(reader.source(), usage);
            }
        }
        rows.rows
    }

    /// The rows add each fold up by day, source and model, in the order they
    /// were first seen. What one entry of a real CLI's transcript folds to is
    /// pinned next to that CLI's reader.
    #[test]
    fn usage_sums_per_day_source_and_model() {
        let rows = rows_of(
            &TestReader(Source::Claude),
            json!([
                { "usage": { "date": "2026-07-20", "model": "claude-fable-5", "input": 10, "output": 5, "cache_read": 100, "cache_write": 20 } },
                { "usage": { "date": "2026-07-20", "model": "claude-fable-5", "input": 2, "output": 3 } },
                // A different model on the same day is its own row, and so is
                // the same model on the next day.
                { "usage": { "date": "2026-07-20", "model": "claude-opus-4-8", "input": 7, "output": 1 } },
                { "usage": { "date": "2026-07-21", "model": "claude-fable-5", "input": 4 } },
                // An entry the fold has nothing for adds nothing.
                { "type": "user" }
            ]),
        );

        let keys: Vec<(&str, &str)> = rows.iter().map(|row| (row.date.as_str(), row.model.as_str())).collect();
        assert_eq!(
            keys,
            [("2026-07-20", "claude-fable-5"), ("2026-07-20", "claude-opus-4-8"), ("2026-07-21", "claude-fable-5")]
        );
        assert!(rows.iter().all(|row| row.source == Source::Claude));
        assert_eq!(
            rows[0].bucket,
            Bucket {
                input: 12.0,
                output: 8.0,
                cache_read: 100.0,
                cache_write: 20.0
            }
        );
        assert_eq!(rows[1].bucket.input, 7.0);
        assert_eq!(rows[2].bucket.input, 4.0);
    }

    #[test]
    fn normalize_days_clamps_to_a_sane_window() {
        assert_eq!(normalize_days(None), 30);
        assert_eq!(normalize_days(Some(0)), 30);
        assert_eq!(normalize_days(Some(7)), 7);
        assert_eq!(normalize_days(Some(9999)), 365);
    }

    #[test]
    fn civil_dates_round_trip() {
        for date in ["1970-01-01", "2000-02-29", "2026-07-20", "2026-03-01", "2099-12-31"] {
            let millis = millis_from_date(date).expect("parsed");
            assert_eq!(date_from_millis(millis), date, "{date} did not survive the round trip");
        }
        assert_eq!(millis_from_date("1970-01-02"), Some(DAY_MS));
        assert_eq!(millis_from_date("not-a-date"), None);
    }

    #[test]
    fn the_window_includes_today_and_reaches_back_n_minus_one_days() {
        assert_eq!(window_bounds("2026-07-20", 1).0, "2026-07-20");
        assert_eq!(window_bounds("2026-03-01", 2).0, "2026-02-28", "2026 is not a leap year");
        assert_eq!(window_bounds("2026-01-01", 30).0, "2025-12-03");
    }

    #[test]
    fn prices_fall_back_to_a_prefix_match() {
        let prices = PriceTable::defaults();
        assert_eq!(prices.get("claude-fable-5").expect("exact").output, 50.0);
        let dated = prices.get("claude-haiku-4-5-20251001").expect("prefix match");
        assert_eq!(dated.input, 1.0);
        assert!(prices.get("gpt-5.6-sol").is_none(), "Codex models are deliberately unpriced");
    }

    /// A model missing from the table costs nothing, which reads as a free
    /// month rather than as a gap — so every generation the user can actually
    /// run has to be in it.
    #[test]
    fn the_current_generation_is_priced() {
        let prices = PriceTable::defaults();
        let opus_5 = prices.get("claude-opus-5").expect("opus 5 is priced");
        assert_eq!(
            opus_5,
            prices.get("claude-opus-4-8").expect("opus 4.8 is priced"),
            "the opus tier's rate did not move with the generation"
        );
        assert_eq!((opus_5.input, opus_5.output), (5.0, 25.0));
        assert_eq!(
            prices.get("claude-opus-5-20260401").map(|price| price.input),
            Some(5.0),
            "a dated id still prices, through the prefix match"
        );
        let fable = prices.get("claude-fable-5").expect("fable is priced");
        assert_eq!((fable.input, fable.output), (10.0, 50.0));
        assert_eq!(fable, prices.get("claude-mythos-5").expect("mythos is priced"));
        let sonnet = prices.get("claude-sonnet-5").expect("sonnet is priced");
        assert_eq!(
            (sonnet.input, sonnet.output),
            (3.0, 15.0),
            "the introductory rate expired on 2026-08-31; this is the standing one"
        );
        let haiku = prices.get("claude-haiku-4-5").expect("haiku is priced");
        assert_eq!((haiku.input, haiku.output), (1.0, 5.0));
    }

    /// One byte of noise in the middle of a rollout used to end the fold there,
    /// silently dropping every turn after it from the month's totals.
    #[tokio::test]
    async fn a_bad_byte_mid_file_does_not_truncate_the_totals() {
        use std::io::Write;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let mut file = std::fs::File::create(&path).expect("create");
        writeln!(file, r#"{{"usage":{{"date":"2026-07-20","model":"m","input":1}}}}"#).expect("write");
        file.write_all(b"{\"type\":\"assistant\",\"note\":\"\xff\"}\n").expect("write");
        writeln!(file, r#"{{"usage":{{"date":"2026-07-20","model":"m","input":4}}}}"#).expect("write");
        drop(file);

        let rows = parse_file(&path, &TestReader(Source::Claude)).await.expect("parse");
        let total: f64 = rows.iter().map(|row| row.bucket.input).sum();
        assert_eq!(total, 5.0, "the turns after the bad byte must still be counted");
    }

    #[test]
    fn cost_is_per_million_tokens() {
        let bucket = Bucket {
            input: 1_000_000.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
        };
        let price = PriceTable::defaults().get("claude-sonnet-5").expect("priced");
        assert_eq!(cost_of(bucket, price), 3.0);
    }

    /// A table no default rate uses, so applying it is unmistakable.
    const CUSTOM_TABLE: &str = r#"{"claude-fable-5":{"input":123.0}}"#;

    /// The rate [`PriceTable::load`] would price `model` at, for the config
    /// folder `paths` names.
    async fn loaded_input(paths: &Paths, model: &str) -> f64 {
        PriceTable::load(&PriceOverride::of(paths).path())
            .await
            .get(model)
            .expect("the model is priced")
            .input
    }

    /// A file in `~/.remote-hand`, as releases before the move wrote it.
    fn legacy_price_file(home: &Path, contents: &str) -> PathBuf {
        let dir = home.join(".remote-hand");
        std::fs::create_dir_all(&dir).expect("legacy folder");
        let path = dir.join("model-prices.json");
        std::fs::write(&path, contents).expect("legacy table");
        path
    }

    #[tokio::test]
    async fn the_old_price_table_is_moved_into_the_config_folder_and_applied() {
        let home = tempfile::tempdir().expect("tempdir");
        legacy_price_file(home.path(), CUSTOM_TABLE);
        let paths = Paths::new(home.path().to_path_buf());

        let prices = PriceOverride::of(&paths);
        assert_eq!(prices.path(), home.path().join(".config/remote-hand/model-prices.json"));
        assert_eq!(prices.migrate_legacy().await, PriceMigration::Moved);

        assert!(prices.path().is_file(), "the table is now in the config folder");
        assert!(!home.path().join(".remote-hand").exists(), "the empty old folder went with it");
        assert_eq!(loaded_input(&paths, "claude-fable-5").await, 123.0, "the reader applies the moved table");
    }

    #[tokio::test]
    async fn the_config_folders_table_wins_and_the_old_one_is_left_alone() {
        let home = tempfile::tempdir().expect("tempdir");
        let old = legacy_price_file(home.path(), CUSTOM_TABLE);
        let paths = Paths::new(home.path().to_path_buf());
        let new = PriceOverride::of(&paths).path();
        std::fs::create_dir_all(new.parent().expect("config dir")).expect("config dir");
        std::fs::write(&new, r#"{"claude-fable-5":{"input":7.0}}"#).expect("new table");

        assert_eq!(PriceOverride::of(&paths).migrate_legacy().await, PriceMigration::BothPresent);

        assert_eq!(loaded_input(&paths, "claude-fable-5").await, 7.0, "the new file wins");
        assert!(old.is_file(), "the old file is left exactly where it was");
        assert_eq!(std::fs::read_to_string(&old).expect("old table"), CUSTOM_TABLE);
    }

    #[tokio::test]
    async fn no_table_at_all_leaves_the_built_in_rates() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());

        assert_eq!(PriceOverride::of(&paths).migrate_legacy().await, PriceMigration::Nothing);
        assert_eq!(loaded_input(&paths, "claude-fable-5").await, 10.0);
        assert!(!home.path().join(".config").exists(), "a boot with nothing to move creates nothing");
    }

    /// `DROPPEN_CONFIG_DIR` moves the folder the table belongs in; the move
    /// follows it, or a second install would keep pricing from the first one's
    /// config folder.
    #[tokio::test]
    async fn a_config_dir_override_is_where_the_moved_table_lands() {
        static ENV_LOCK: Mutex<()> = Mutex::new(());

        let home = tempfile::tempdir().expect("tempdir");
        let config = tempfile::tempdir().expect("tempdir");
        legacy_price_file(home.path(), CUSTOM_TABLE);
        // The guard is scoped to the environment read: held across the await
        // below it would make this test's future non-`Send`.
        let paths = {
            let _guard = ENV_LOCK.lock().expect("env lock");
            let previous = std::env::var_os("DROPPEN_CONFIG_DIR");
            std::env::set_var("DROPPEN_CONFIG_DIR", config.path());
            let paths = Paths::new(home.path().to_path_buf());
            match previous {
                Some(value) => std::env::set_var("DROPPEN_CONFIG_DIR", value),
                None => std::env::remove_var("DROPPEN_CONFIG_DIR"),
            }
            paths
        };

        let prices = PriceOverride::of(&paths);
        assert_eq!(prices.path(), config.path().join("model-prices.json"));
        assert_eq!(prices.migrate_legacy().await, PriceMigration::Moved);
        assert!(prices.path().is_file(), "the override folder is where it went");
        assert_eq!(loaded_input(&paths, "claude-fable-5").await, 123.0);
    }

    /// The reader treats a table it cannot read as no table: a broken file
    /// must not fail the usage route, and must not be silently priced at zero.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_unreadable_price_table_falls_back_to_the_built_in_rates() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let path = PriceOverride::of(&paths).path();
        std::fs::create_dir_all(path.parent().expect("config dir")).expect("config dir");
        std::fs::write(&path, CUSTOM_TABLE).expect("table");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("unreadable");

        let price = PriceTable::load(&path).await.get("claude-fable-5").expect("priced");
        assert_eq!(price.input, 10.0);

        // Let the tempdir go without leaving an unreadable file behind.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("readable again");
    }
}
