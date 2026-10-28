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

use crate::openapi_ext::Nullable;
use crate::sessions::util::{try_parse, LossyLines};
use crate::sessions::{iso8601, mtime_ms, Paths, Source};

/// One parsed JSONL entry.
type Entry = serde_json::Map<String, Value>;

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
}

/// A finite JSON number, or zero — Node's `typeof v === "number" && isFinite(v)`.
fn num(value: Option<&Value>) -> f64 {
    value.and_then(Value::as_f64).filter(|n| n.is_finite()).unwrap_or(0.0)
}

/// UTC calendar day of an entry's `timestamp`, or `None` when it has none.
fn date_of(entry: &Entry) -> Option<String> {
    entry
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|ts| ts.get(..10))
        .map(str::to_owned)
}

/// Claude assistant entries carry per-message `message.usage` + `message.model`.
fn fold_claude(rows: &mut RowMap, entry: &Entry) {
    if entry.get("type").and_then(Value::as_str) != Some("assistant") {
        return;
    }
    let Some(message) = entry.get("message").and_then(Value::as_object) else {
        return;
    };
    let Some(usage) = message.get("usage").and_then(Value::as_object) else {
        return;
    };
    let Some(date) = date_of(entry) else { return };
    let delta = Bucket {
        input: num(usage.get("input_tokens")),
        output: num(usage.get("output_tokens")),
        cache_read: num(usage.get("cache_read_input_tokens")),
        cache_write: num(usage.get("cache_creation_input_tokens")),
    };
    if delta.total() == 0.0 {
        return;
    }
    let model = message.get("model").and_then(Value::as_str).unwrap_or("unknown");
    rows.add(&date, Source::Claude, model, delta);
}

/// Codex emits *cumulative* `token_count` events; the per-turn delta lives in
/// `last_token_usage`, and the active model comes from the preceding
/// `turn_context`. Events can repeat, so only a strict increase of the running
/// total is counted.
#[derive(Debug)]
struct CodexFolder {
    model: String,
    prev_total: f64,
}

impl Default for CodexFolder {
    fn default() -> Self {
        Self {
            model: "codex".to_string(),
            prev_total: 0.0,
        }
    }
}

impl CodexFolder {
    fn fold(&mut self, rows: &mut RowMap, entry: &Entry) {
        let entry_type = entry.get("type").and_then(Value::as_str);
        if entry_type == Some("turn_context") {
            let model = entry
                .get("payload")
                .and_then(Value::as_object)
                .and_then(|payload| payload.get("model"))
                .and_then(Value::as_str)
                .filter(|model| !model.is_empty());
            if let Some(model) = model {
                self.model = model.to_string();
            }
            return;
        }
        if entry_type != Some("event_msg") {
            return;
        }
        let Some(payload) = entry.get("payload").and_then(Value::as_object) else {
            return;
        };
        if payload.get("type").and_then(Value::as_str) != Some("token_count") {
            return;
        }
        let Some(info) = payload.get("info").and_then(Value::as_object) else {
            return;
        };
        let total = info
            .get("total_token_usage")
            .and_then(Value::as_object)
            .map(|usage| num(usage.get("total_tokens")))
            .unwrap_or(0.0);
        if total <= self.prev_total {
            return; // duplicate / non-progressing report
        }
        self.prev_total = total;
        let Some(date) = date_of(entry) else { return };
        let Some(last) = info.get("last_token_usage").and_then(Value::as_object) else {
            return;
        };
        let cache_read = num(last.get("cached_input_tokens"));
        let delta = Bucket {
            // `input_tokens` already includes whatever came out of the cache.
            input: (num(last.get("input_tokens")) - cache_read).max(0.0),
            output: num(last.get("output_tokens")),
            cache_read,
            cache_write: 0.0,
        };
        rows.add(&date, Source::Codex, &self.model, delta);
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
/// extend any of this by writing `~/.remote-hand/model-prices.json` (same shape,
/// keyed by model id).
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

    /// The built-in table with `~/.remote-hand/model-prices.json` merged over it.
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

/// A session file inside the window, with the mtime its parse is cached on.
#[derive(Debug, Clone)]
struct FileRef {
    path: PathBuf,
    mtime: f64,
}

/// One file's folded rows, keyed by the mtime they were folded from.
type CachedRows = HashMap<PathBuf, (f64, Vec<UsageRow>)>;

/// Parsed rows per file+mtime, so a refresh only re-reads sessions that changed.
static FILE_CACHE: LazyLock<Mutex<CachedRows>> = LazyLock::new(|| Mutex::new(HashMap::new()));

async fn stat_ref(path: PathBuf, cutoff_ms: f64) -> Option<FileRef> {
    let metadata = tokio::fs::metadata(&path).await.ok()?;
    let mtime = mtime_ms(&metadata);
    (mtime >= cutoff_ms).then_some(FileRef { path, mtime })
}

fn is_jsonl(path: &Path) -> bool {
    path.file_name().map(|name| name.to_string_lossy().ends_with(".jsonl")).unwrap_or(false)
}

/// Claude keeps one flat `<uuid>.jsonl` per session under each project dir, and
/// both config roots contribute: usage is per machine, not per login.
async fn claude_files(paths: &Paths, cutoff_ms: f64) -> Vec<FileRef> {
    let mut out = Vec::new();
    for (_, projects) in paths.claude_projects_dirs() {
        let Ok(mut roots) = tokio::fs::read_dir(&projects).await else { continue };
        while let Ok(Some(project)) = roots.next_entry().await {
            let Ok(mut files) = tokio::fs::read_dir(project.path()).await else {
                continue;
            };
            while let Ok(Some(file)) = files.next_entry().await {
                let path = file.path();
                if !is_jsonl(&path) {
                    continue;
                }
                if let Some(reference) = stat_ref(path, cutoff_ms).await {
                    out.push(reference);
                }
            }
        }
    }
    out
}

/// Codex nests rollouts under `sessions/YYYY/MM/DD`, so the tree is walked.
///
/// An explicit stack rather than recursion: a recursive `async fn` needs its
/// future boxed at every level, which buys nothing here.
async fn codex_files(paths: &Paths, cutoff_ms: f64) -> Vec<FileRef> {
    let mut out = Vec::new();
    let mut pending = vec![paths.codex_sessions()];
    while let Some(dir) = pending.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else { continue };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let Ok(file_type) = entry.file_type().await else { continue };
            if file_type.is_dir() {
                pending.push(path);
            } else if is_jsonl(&path) {
                if let Some(reference) = stat_ref(path, cutoff_ms).await {
                    out.push(reference);
                }
            }
        }
    }
    out
}

/// Stream a single session file, holding at most one line in memory at a time.
async fn parse_file(path: &Path, source: Source) -> std::io::Result<Vec<UsageRow>> {
    let mut lines = LossyLines::open(path).await?;
    let mut rows = RowMap::default();
    let mut codex = CodexFolder::default();
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
        match source {
            Source::Claude => fold_claude(&mut rows, &entry),
            Source::Codex => codex.fold(&mut rows, &entry),
            // An ACP agent reports no per-model token counts this server can price.
            Source::Acp => {}
        }
    }
    Ok(rows.rows)
}

async fn file_rows(file: FileRef, source: Source) -> Vec<UsageRow> {
    if let Some((mtime, rows)) = FILE_CACHE.lock().expect("usage cache poisoned").get(&file.path) {
        if *mtime == file.mtime {
            return rows.clone();
        }
    }
    // An unreadable file contributes nothing and is not cached, so a transient
    // failure does not stick until the file changes again.
    let Ok(rows) = parse_file(&file.path, source).await else {
        return Vec::new();
    };
    FILE_CACHE
        .lock()
        .expect("usage cache poisoned")
        .insert(file.path.clone(), (file.mtime, rows.clone()));
    rows
}

async fn rows_per_file(refs: Vec<FileRef>, source: Source) -> Vec<Vec<UsageRow>> {
    futures_util::stream::iter(refs)
        .map(|file| file_rows(file, source))
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

/// Aggregate token usage across every Claude and Codex session touched within
/// the last `days` days, grouped by calendar day and by model.
pub async fn collect_usage(paths: &Paths, days: Option<u32>) -> UsageStats {
    let window_days = normalize_days(days);
    let generated_at = iso8601(SystemTime::now());
    let today = generated_at[..10].to_string();
    let (since, cutoff_ms) = window_bounds(&today, window_days);
    let cutoff_ms = cutoff_ms as f64;

    let (claude_refs, codex_refs) = tokio::join!(claude_files(paths, cutoff_ms), codex_files(paths, cutoff_ms));
    let (claude_per_file, codex_per_file) = tokio::join!(rows_per_file(claude_refs, Source::Claude), rows_per_file(codex_refs, Source::Codex));

    let mut acc = Accumulator::default();
    let claude_sessions = acc.consume(&claude_per_file, &since);
    let codex_sessions = acc.consume(&codex_per_file, &since);

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
    let prices = PriceTable::load(&paths.price_override()).await;
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

    let by_source = [(Source::Claude, claude_sessions), (Source::Codex, codex_sessions)]
        .into_iter()
        .map(|(source, sessions)| {
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
            sessions: claude_sessions + codex_sessions,
            cost_usd,
        },
        daily,
        by_model,
        by_source,
        pricing: UsagePricing {
            unpriced_models,
            override_path: paths.price_override().to_string_lossy().into_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Fold a literal transcript the way `parse_file` folds a real one.
    fn claude_rows(entries: Value) -> Vec<UsageRow> {
        let mut rows = RowMap::default();
        for entry in entries.as_array().expect("array") {
            fold_claude(&mut rows, entry.as_object().expect("object"));
        }
        rows.rows
    }

    fn codex_rows(entries: Value) -> Vec<UsageRow> {
        let mut rows = RowMap::default();
        let mut folder = CodexFolder::default();
        for entry in entries.as_array().expect("array") {
            folder.fold(&mut rows, entry.as_object().expect("object"));
        }
        rows.rows
    }

    #[test]
    fn claude_usage_sums_per_day_and_model_ignoring_non_usage_entries() {
        let rows = claude_rows(json!([
            {
                "type": "assistant",
                "timestamp": "2026-07-20T10:00:00.000Z",
                "message": {
                    "model": "claude-fable-5",
                    "usage": { "input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 100, "cache_creation_input_tokens": 20 }
                }
            },
            {
                "type": "assistant",
                "timestamp": "2026-07-20T11:00:00.000Z",
                "message": {
                    "model": "claude-fable-5",
                    "usage": { "input_tokens": 2, "output_tokens": 3, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0 }
                }
            },
            // A different model on the same day is its own row.
            {
                "type": "assistant",
                "timestamp": "2026-07-20T12:00:00.000Z",
                "message": { "model": "claude-opus-4-8", "usage": { "input_tokens": 7, "output_tokens": 1 } }
            },
            // A user turn and a usage-less assistant entry are ignored.
            { "type": "user", "timestamp": "2026-07-20T10:30:00.000Z", "message": { "content": "hi" } },
            { "type": "assistant", "timestamp": "2026-07-20T13:00:00.000Z", "message": { "model": "claude-fable-5" } }
        ]));

        assert_eq!(rows.len(), 2);
        let fable = rows.iter().find(|row| row.model == "claude-fable-5").expect("fable row");
        assert_eq!(fable.date, "2026-07-20");
        assert_eq!(fable.source, Source::Claude);
        assert_eq!(
            fable.bucket,
            Bucket {
                input: 12.0,
                output: 8.0,
                cache_read: 100.0,
                cache_write: 20.0
            }
        );
        let opus = rows.iter().find(|row| row.model == "claude-opus-4-8").expect("opus row");
        assert_eq!(opus.bucket.input, 7.0);
    }

    #[test]
    fn claude_usage_splits_the_same_model_across_calendar_days() {
        let rows = claude_rows(json!([
            { "type": "assistant", "timestamp": "2026-07-20T23:59:00.000Z", "message": { "model": "m", "usage": { "input_tokens": 1 } } },
            { "type": "assistant", "timestamp": "2026-07-21T00:01:00.000Z", "message": { "model": "m", "usage": { "input_tokens": 4 } } }
        ]));
        let mut seen: Vec<(String, f64)> = rows.iter().map(|row| (row.date.clone(), row.bucket.input)).collect();
        seen.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(seen, vec![("2026-07-20".to_string(), 1.0), ("2026-07-21".to_string(), 4.0)]);
    }

    #[test]
    fn a_usage_less_or_zero_total_assistant_entry_is_skipped() {
        let rows = claude_rows(json!([
            { "type": "assistant", "timestamp": "2026-07-20T10:00:00.000Z", "message": { "model": "m", "usage": { "input_tokens": 0 } } }
        ]));
        assert!(rows.is_empty(), "a zero-total delta adds nothing");
    }

    #[test]
    fn an_entry_without_a_model_is_attributed_to_unknown() {
        let rows = claude_rows(json!([
            { "type": "assistant", "timestamp": "2026-07-20T10:00:00.000Z", "message": { "usage": { "input_tokens": 3 } } }
        ]));
        assert_eq!(rows[0].model, "unknown");
    }

    #[test]
    fn codex_usage_counts_per_turn_deltas_and_attributes_the_active_model() {
        let rows = codex_rows(json!([
            { "type": "turn_context", "timestamp": "2026-07-22T02:00:00.000Z", "payload": { "model": "gpt-5.6-sol" } },
            {
                "type": "event_msg",
                "timestamp": "2026-07-22T02:01:00.000Z",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": { "total_tokens": 9958 },
                        "last_token_usage": { "input_tokens": 9865, "cached_input_tokens": 7552, "output_tokens": 93 }
                    }
                }
            },
            {
                "type": "event_msg",
                "timestamp": "2026-07-22T02:02:00.000Z",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": { "total_tokens": 22391 },
                        "last_token_usage": { "input_tokens": 12395, "cached_input_tokens": 7552, "output_tokens": 38 }
                    }
                }
            }
        ]));

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].date, "2026-07-22");
        assert_eq!(rows[0].source, Source::Codex);
        assert_eq!(rows[0].model, "gpt-5.6-sol");
        assert_eq!(
            rows[0].bucket,
            Bucket {
                // fresh input = (9865-7552) + (12395-7552)
                input: 7156.0,
                output: 131.0,
                cache_read: 15104.0,
                cache_write: 0.0,
            }
        );
    }

    #[test]
    fn codex_usage_ignores_duplicate_token_count_reports() {
        let repeated = json!({
            "type": "event_msg",
            "timestamp": "2026-07-22T02:01:00.000Z",
            "payload": {
                "type": "token_count",
                "info": { "total_token_usage": { "total_tokens": 100 }, "last_token_usage": { "input_tokens": 100, "output_tokens": 0 } }
            }
        });
        let rows = codex_rows(json!([repeated, repeated]));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].bucket.input, 100.0);
        assert_eq!(rows[0].model, "codex", "the default until a turn_context is seen");
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
        writeln!(
            file,
            r#"{{"type":"assistant","timestamp":"2026-07-20T10:00:00.000Z","message":{{"model":"m","usage":{{"input_tokens":1}}}}}}"#
        )
        .expect("write");
        file.write_all(b"{\"type\":\"assistant\",\"note\":\"\xff\"}\n").expect("write");
        writeln!(
            file,
            r#"{{"type":"assistant","timestamp":"2026-07-20T11:00:00.000Z","message":{{"model":"m","usage":{{"input_tokens":4}}}}}}"#
        )
        .expect("write");
        drop(file);

        let rows = parse_file(&path, Source::Claude).await.expect("parse");
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
}
