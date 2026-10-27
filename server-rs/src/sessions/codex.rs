//! Codex session discovery and transcripts.
//!
//! Port of the former TypeScript server's `codex.ts`. Discovery has two
//! independent paths and the fallback is not a degraded copy of the first:
//!
//! * **The state database.** Codex keeps a thread index in
//!   `~/.codex/state_5.sqlite`, and it is preferred because it is the only place
//!   that knows which threads are archived or belong to a subagent — both of
//!   which must stay hidden. It also carries the user-visible title and the
//!   git branch, neither of which appears in a rollout file.
//! * **Scanning `~/.codex/sessions`.** When the database is missing or its
//!   schema has moved on, every `rollout-*.jsonl` is walked instead and a title
//!   is derived from the first real user message, with `session_index.jsonl`
//!   supplying thread names when it exists.
//!
//! Neither path can report a live process: Codex has no pid registry, so
//! "running" is inferred from how recently the transcript was written and what
//! the last entry in it was.
//!
//! Assistant file mentions use the same existence and home-directory checks as
//! Claude transcripts, including Codex's structured file-citation directives.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde_json::Value;

use super::codex_claims::{Claims, Verdict};
use super::codex_tools::{CodexTools, Context};
use crate::time::now_ms;
use super::transcript::title_line;
use super::util::{make_item, parse_attachment_suffix, read_head_lines, read_tail_lines, try_parse, ItemKind, LossyLines, Role, TranscriptItem};
use super::{iso8601, mtime_ms, owned_sessions, CliChoice, Paths, SessionActivity, SessionScope, SessionStatus, SessionSummary, Source};
use crate::openapi_ext::Nullable;

/// Codex has no reliable live-process registry, so anything untouched for this
/// long counts as idle; recent files get their tail inspected.
const ACTIVE_WINDOW_MS: i64 = 15 * 60 * 1000;
/// The first `session_meta` line can exceed 100 KiB (embedded base instructions).
const HEAD_BYTES: usize = 512 * 1024;
/// `readTailLines`' default in the former TypeScript server's `util.ts`.
const TAIL_BYTES: u64 = 64 * 1024;
/// Codex only writes `turn_context` at each turn start, and a turn's output can
/// run long, so the model/effort reading needs a much deeper tail than Claude's.
const CHOICE_TAIL_BYTES: u64 = 1024 * 1024;


/// Is this session doing anything, judged from the tail of its transcript?
async fn codex_status(file_path: &Path, updated_ms: i64) -> SessionStatus {
    if now_ms() - updated_ms > ACTIVE_WINDOW_MS {
        return SessionStatus::Idle;
    }
    let Ok(lines) = read_tail_lines(file_path, TAIL_BYTES).await else {
        return SessionStatus::Idle;
    };
    for line in lines.iter().rev() {
        let Some(entry) = try_parse(line) else { continue };
        let Some(payload) = entry.get("payload").and_then(Value::as_object) else {
            continue;
        };
        let kind = payload.get("type").and_then(Value::as_str);
        let role = payload.get("role").and_then(Value::as_str);
        match entry.get("type").and_then(Value::as_str) {
            Some("event_msg") => match kind {
                Some("agent_message") | Some("turn_aborted") | Some("task_complete") => return SessionStatus::Waiting,
                Some("user_message") | Some("task_started") => return SessionStatus::Running,
                // token_count and friends carry no signal.
                _ => continue,
            },
            Some("response_item") => {
                return match kind {
                    Some("message") if role == Some("assistant") => SessionStatus::Waiting,
                    Some("message") => SessionStatus::Running,
                    // Tool calls, outputs, reasoning: still working.
                    _ => SessionStatus::Running,
                };
            }
            _ => continue,
        }
    }
    SessionStatus::Idle
}

/// Apply the inferred status to a freshly built summary.
async fn with_codex_status(mut summary: SessionSummary, updated_ms: i64) -> SessionSummary {
    summary.status = codex_status(Path::new(&summary.file_path), updated_ms).await;
    summary
}

/// What one rollout file says about itself, cached by mtime and size.
///
/// `Serialize`/`Deserialize` because this is the payload
/// [`crate::db::index`] stores: what survives a restart is this struct, not the
/// `SessionSummary` built from it, because the summary also carries a thread
/// name and an ownership verdict that come from elsewhere and can change while
/// the rollout file does not move at all.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct FileMeta {
    id: String,
    file_path: PathBuf,
    cwd: Option<String>,
    title_fallback: Option<String>,
    updated_ms: i64,
    updated_at: String,
    created_at: String,
    size_bytes: u64,
}

/// How many rollout files the in-memory tier holds before it is emptied.
///
/// The same unbounded-growth fix, and the same reasoning, as `claude.rs`'s cap
/// of the same name: nothing ever dropped the entry for a rollout
/// that had been deleted, and a clear now costs a `stat` and an index lookup
/// rather than a re-read of a 512 KiB head.
const CACHE_MAX: usize = 2048;

/// The near tier: what this process has already read, keyed by rollout path.
/// Behind it is [`crate::db::index`], which remembers the same thing across a
/// restart.
static META_CACHE: LazyLock<Mutex<HashMap<PathBuf, (f64, FileMeta)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Hold a rollout's metadata in the near tier, keeping that tier bounded.
fn remember(file_path: &Path, mtime: f64, meta: &FileMeta) {
    let mut cache = META_CACHE.lock().expect("codex meta cache poisoned");
    // Never let a refresh of a file already held trigger the clear; at the cap
    // that would empty the cache on every list and never hit it again.
    if cache.len() >= CACHE_MAX && !cache.contains_key(file_path) {
        cache.clear();
    }
    cache.insert(file_path.to_path_buf(), (mtime, meta.clone()));
}

/// `rollout-2026-05-09T16-39-49-<uuid>.jsonl` -> `<uuid>`
fn id_from_file_name(name: &str) -> Option<String> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"^rollout-.*-([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\.jsonl$").expect("valid rollout pattern")
    });
    re.captures(name).map(|caps| caps[1].to_string())
}

/// Every `*.jsonl` under `dir`, however deeply nested.
///
/// Codex files rollouts under `sessions/YYYY/MM/DD`, and the depth is its
/// business, not ours. Iterative rather than recursive: an `async fn` that calls
/// itself needs boxing, and a worklist says the same thing with less ceremony.
async fn collect_session_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    let mut pending: Vec<PathBuf> = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&current).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Ok(kind) = entry.file_type().await else { continue };
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() && name.ends_with(".jsonl") {
                out.push((name, entry.path()));
            }
        }
    }
    out
}

/// Whether a message is Codex's own injected preamble rather than the user's words.
pub(crate) fn is_wrapper_text(text: &str) -> bool {
    const PREFIXES: [&str; 8] = [
        "# AGENTS.md instructions",
        "<codex_internal_context",
        "<recommended_plugins",
        "<environment_context>",
        "<user_instructions>",
        "<turn_context>",
        "<permissions",
        "<app_context",
    ];
    let trimmed = text.trim_start();
    PREFIXES.iter().any(|prefix| trimmed.starts_with(prefix))
}

/// Read a rollout file's head for its working directory and a fallback title.
async fn file_meta(file_path: &Path, id: &str) -> std::io::Result<FileMeta> {
    let metadata = tokio::fs::metadata(file_path).await?;
    let mtime = mtime_ms(&metadata);
    let mark = crate::db::index::Watermark::of(file_path, &metadata);
    if let Some((cached_mtime, meta)) = META_CACHE.lock().expect("codex meta cache poisoned").get(file_path) {
        // Size too, so this tier and the durable one agree on what "unchanged"
        // means; the metadata already carries the length it was read at.
        if *cached_mtime == mtime && meta.size_bytes == metadata.len() {
            return Ok(meta.clone());
        }
    }

    // What a previous boot read out of this exact file. A rollout's head can be
    // half a megabyte — the `session_meta` line alone carries the base
    // instructions — so this is the read worth not repeating.
    let index = crate::db::index::current();
    if let Some(meta) = index.as_ref().and_then(|index| index.summary::<FileMeta>(Source::Codex, id, mark)) {
        remember(file_path, mtime, &meta);
        return Ok(meta);
    }

    let mut cwd: Option<String> = None;
    let mut title_fallback: Option<String> = None;
    let mut created_at: Option<String> = None;
    for line in read_head_lines(file_path, HEAD_BYTES).await? {
        let Some(entry) = try_parse(&line) else { continue };
        // Read before the payload gate below: the rollout's opening entries are
        // what date it, whether or not they carry a payload we care about.
        if created_at.is_none() {
            created_at = entry
                .get("timestamp")
                .and_then(Value::as_str)
                .filter(|stamp| !stamp.is_empty())
                .map(str::to_owned);
        }
        let Some(payload) = entry.get("payload").and_then(Value::as_object) else {
            continue;
        };
        match entry.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                if let Some(value) = payload.get("cwd").and_then(Value::as_str) {
                    cwd = Some(value.to_string());
                }
            }
            Some("response_item")
                if payload.get("type").and_then(Value::as_str) == Some("message") && payload.get("role").and_then(Value::as_str) == Some("user") =>
            {
                let parts = payload.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
                for part in parts {
                    let Some(text) = part.get("text").and_then(Value::as_str) else {
                        continue;
                    };
                    if !text.trim().is_empty() && !is_wrapper_text(text) {
                        title_fallback = Some(text.to_string());
                        break;
                    }
                }
            }
            _ => {}
        }
        if cwd.is_some() && title_fallback.is_some() {
            break;
        }
    }

    let meta = FileMeta {
        id: id.to_string(),
        file_path: file_path.to_path_buf(),
        cwd,
        title_fallback,
        updated_ms: mtime as i64,
        updated_at: metadata.modified().map(iso8601).unwrap_or_default(),
        created_at: created_at.unwrap_or_else(|| metadata.created().or_else(|_| metadata.modified()).map(iso8601).unwrap_or_default()),
        size_bytes: metadata.len(),
    };
    remember(file_path, mtime, &meta);
    if let Some(index) = index {
        index.remember_summary(Source::Codex, id, mark, &meta);
    }
    Ok(meta)
}

/// Thread names from the optional `session_index.jsonl`.
async fn thread_names(paths: &Paths) -> HashMap<String, String> {
    let mut names = HashMap::new();
    // The index is optional; without it sessions fall back to their first message.
    let Ok(raw) = tokio::fs::read_to_string(paths.codex_session_index()).await else {
        return names;
    };
    for line in raw.split('\n') {
        let Some(entry) = try_parse(line) else { continue };
        let (Some(id), Some(name)) = (entry.get("id").and_then(Value::as_str), entry.get("thread_name").and_then(Value::as_str)) else {
            continue;
        };
        names.insert(id.to_string(), name.to_string());
    }
    names
}

/// One visible row of the `threads` table.
#[derive(Debug, Clone)]
struct ThreadRow {
    id: String,
    title: Option<String>,
    first_user_message: Option<String>,
    cwd: Option<String>,
    git_branch: Option<String>,
    updated_at: i64,
    updated_at_ms: Option<i64>,
    rollout_path: String,
    created_at: i64,
}

/// The columns and the visibility filter both queries share.
///
/// `source NOT LIKE '{%'` is Codex's own way of marking a subagent thread: its
/// `source` column holds a JSON blob for those and a plain word otherwise.
const THREAD_COLUMNS: &str =
    "SELECT id, title, first_user_message, cwd, git_branch, updated_at, updated_at_ms, rollout_path, created_at FROM threads";

fn read_thread_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ThreadRow> {
    Ok(ThreadRow {
        id: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
        title: row.get(1)?,
        first_user_message: row.get(2)?,
        cwd: row.get(3)?,
        git_branch: row.get(4)?,
        updated_at: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
        updated_at_ms: row.get(6)?,
        rollout_path: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
        created_at: row.get::<_, Option<i64>>(8)?.unwrap_or(0),
    })
}

/// Open the state database read-only, or `None` when there is not one.
fn open_state_db(db: &Path) -> Option<rusqlite::Connection> {
    rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI).ok()
}

/// Every visible thread, newest first, or `None` when the database is missing or
/// incompatible — which is the signal for the caller to fall back to scanning.
///
/// A row that will not convert is dropped rather than failing the whole query:
/// one odd row should cost one session, not the entire preferred discovery path.
fn list_from_state_db(db: &Path) -> Option<Vec<ThreadRow>> {
    let connection = open_state_db(db)?;
    let mut statement = connection
        .prepare(&format!(
            "{THREAD_COLUMNS} WHERE archived = 0 AND source NOT LIKE '{{%' ORDER BY updated_at DESC"
        ))
        .ok()?;
    let rows = statement.query_map([], read_thread_row).ok()?;
    Some(rows.filter_map(Result::ok).collect())
}

/// `None` means the database is unavailable (fall back to scanning); `Some(None)`
/// means the thread exists but must stay hidden, or is simply unknown.
fn query_thread_row(db: &Path, id: &str) -> Option<Option<ThreadRow>> {
    let connection = open_state_db(db)?;
    let mut statement = connection
        .prepare(&format!("{THREAD_COLUMNS} WHERE id = ? AND archived = 0 AND source NOT LIKE '{{%'"))
        .ok()?;
    match statement.query_row([id], read_thread_row) {
        Ok(row) => Some(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Some(None),
        Err(_) => None,
    }
}

/// A delegated subagent thread: a row of Codex's `thread_spawn_edges`, joined
/// to the thread it names.
///
/// The edge table is the parent link Codex itself keeps. A thread whose
/// `source` says subagent but which has no edge — Codex's own guardian review,
/// which no conversation delegated — is not one of these, and stays hidden.
#[derive(Debug, Clone)]
pub(super) struct SpawnedThread {
    pub(super) id: String,
    pub(super) parent_id: String,
    /// The name Codex gave it (`agent_nickname`), or its title.
    pub(super) name: String,
    /// What it was spawned as (`agent_role`), when Codex says.
    pub(super) role: Option<String>,
    pub(super) rollout_path: String,
    pub(super) updated_ms: i64,
}

/// The parent of every delegated subagent thread, or nothing without the
/// tables: an older Codex, or no Codex at all.
fn spawned_threads_from_state_db(db: &Path) -> Option<Vec<SpawnedThread>> {
    let connection = open_state_db(db)?;
    let mut statement = connection
        .prepare(
            "SELECT t.id, e.parent_thread_id, t.agent_nickname, t.agent_role, t.title, t.rollout_path, t.updated_at, t.updated_at_ms \
             FROM thread_spawn_edges e JOIN threads t ON t.id = e.child_thread_id WHERE t.archived = 0",
        )
        .ok()?;
    let rows = statement
        .query_map([], |row| {
            let nickname: Option<String> = row.get(2)?;
            let title: Option<String> = row.get(4)?;
            let updated_at: i64 = row.get::<_, Option<i64>>(6)?.unwrap_or(0);
            Ok(SpawnedThread {
                id: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                parent_id: row.get(1)?,
                name: nickname
                    .filter(|name| !name.trim().is_empty())
                    .or_else(|| title.map(|title| title_line(&title)))
                    .unwrap_or_else(|| "subagent".to_string()),
                role: row.get(3)?,
                rollout_path: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                updated_ms: row.get::<_, Option<i64>>(7)?.unwrap_or(updated_at * 1000),
            })
        })
        .ok()?;
    Some(rows.filter_map(Result::ok).filter(|thread| !thread.id.is_empty()).collect())
}

/// Every delegated subagent thread Codex knows of.
pub(super) async fn spawned_threads(paths: &Paths) -> Vec<SpawnedThread> {
    on_state_db(paths, spawned_threads_from_state_db).await.flatten().unwrap_or_default()
}

/// How a subagent thread is doing: whether it is still working, and whether
/// its last turn ended in an error or was aborted.
pub(super) async fn spawned_thread_state(thread: &SpawnedThread) -> (bool, bool) {
    let path = Path::new(&thread.rollout_path);
    let failed = match read_tail_lines(path, TAIL_BYTES).await {
        Ok(lines) => lines
            .iter()
            .rev()
            .filter_map(|line| try_parse(line))
            .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("event_msg"))
            .find_map(
                |entry| match entry.get("payload").and_then(|payload| payload.get("type")).and_then(Value::as_str) {
                    Some("error" | "turn_aborted") => Some(true),
                    Some("task_complete" | "task_started" | "agent_message" | "user_message") => Some(false),
                    _ => None,
                },
            )
            .unwrap_or(false),
        Err(_) => false,
    };
    // `codex_status` reads past an `error` to the message before it, which
    // says a turn is in flight; the error is what ended that turn.
    let working = !failed && codex_status(path, thread.updated_ms).await == SessionStatus::Running;
    (working, failed)
}

/// The delegated subagent thread `id`, when it is one: the thread row, and
/// the thread that spawned it.
fn query_spawned_thread_row(db: &Path, id: &str) -> Option<(ThreadRow, String)> {
    let connection = open_state_db(db)?;
    let mut statement = connection
        .prepare(
            // The columns of [`THREAD_COLUMNS`], in its order, so
            // [`read_thread_row`] reads them; the parent is the tenth. Titled
            // by its nickname, as its row under the parent is: Codex leaves a
            // subagent's own title empty.
            "SELECT t.id, COALESCE(NULLIF(t.agent_nickname, ''), t.title), t.first_user_message, t.cwd, t.git_branch, t.updated_at, t.updated_at_ms, t.rollout_path, t.created_at, \
             e.parent_thread_id FROM threads t JOIN thread_spawn_edges e ON e.child_thread_id = t.id WHERE t.id = ? AND t.archived = 0",
        )
        .ok()?;
    statement
        .query_row([id], |row| Ok((read_thread_row(row)?, row.get::<_, String>(9)?)))
        .ok()
}

/// Run a state-database query off the async runtime's worker threads.
///
/// SQLite is a blocking API, and a cold page cache on a large index is long
/// enough to matter to every other request in flight.
async fn on_state_db<T: Send + 'static>(paths: &Paths, query: impl FnOnce(&Path) -> T + Send + 'static) -> Option<T> {
    let db = paths.codex_state_db();
    tokio::task::spawn_blocking(move || query(&db)).await.ok()
}

/// Build a list row from a thread row, returning the update instant separately
/// so status inference does not have to parse the timestamp back.
fn row_to_summary(row: &ThreadRow, size_bytes: u64) -> (SessionSummary, i64) {
    let updated_ms = row.updated_at_ms.unwrap_or(row.updated_at * 1000);
    // A row with no start recorded is dated by its last activity rather than by
    // the epoch, which would sort it below every real session forever.
    let created_ms = if row.created_at > 0 { row.created_at * 1000 } else { updated_ms };
    let title = row
        .title
        .as_deref()
        .filter(|t| !t.is_empty())
        .or(row.first_user_message.as_deref().filter(|t| !t.is_empty()))
        .unwrap_or("(untitled)");
    let summary = SessionSummary {
        pinned: false,
        id: row.id.clone(),
        source: Source::Codex,
        // The pessimistic default; `apply_ownership` promotes the ids this
        // instance has a record of creating and drops the ones another instance
        // claimed. Getting this wrong in the other direction would let two
        // servers drive one thread.
        scope: SessionScope::External,
        // Named by the same pass, which is the only one holding a `Paths`.
        server: String::new(),
        title: title_line(title),
        cwd: Nullable(row.cwd.clone().filter(|c| !c.is_empty())),
        repo_key: Nullable(None),
        git_branch: Nullable(row.git_branch.clone()),
        updated_at: iso8601(std::time::UNIX_EPOCH + std::time::Duration::from_millis(updated_ms.max(0) as u64)),
        created_at: iso8601(std::time::UNIX_EPOCH + std::time::Duration::from_millis(created_ms.max(0) as u64)),
        size_bytes,
        file_path: row.rollout_path.clone(),
        status: SessionStatus::Idle,
        activity: SessionActivity::Idle,
        resident: Nullable(None),
        claude_root: None,
        account_id: None,
        // All filled in by `crate::attention::apply`, once per changed file.
        attention_at: Nullable(None),
        unread: false,
        unread_count: 0,
        // Never set for codex: it runs with approvals off and every interactive
        // request the app server makes is refused (`inject::app_server`), so a
        // codex thread has no way to stop and ask.
        asking: false,
        children: Vec::new(),
        parent_id: Nullable(None),
    };
    (summary, updated_ms)
}

/// Build a list row from a scanned rollout file.
fn meta_to_summary(meta: &FileMeta, names: &HashMap<String, String>) -> SessionSummary {
    let title = names
        .get(&meta.id)
        .map(String::as_str)
        .or(meta.title_fallback.as_deref())
        .unwrap_or("(untitled)");
    SessionSummary {
        id: meta.id.clone(),
        pinned: false,
        source: Source::Codex,
        scope: SessionScope::External,
        // Both settled by `apply_ownership`, as above.
        server: String::new(),
        title: title_line(title),
        cwd: Nullable(meta.cwd.clone()),
        repo_key: Nullable(None),
        git_branch: Nullable(None),
        updated_at: meta.updated_at.clone(),
        created_at: meta.created_at.clone(),
        size_bytes: meta.size_bytes,
        file_path: meta.file_path.to_string_lossy().into_owned(),
        status: SessionStatus::Idle,
        activity: SessionActivity::Idle,
        resident: Nullable(None),
        claude_root: None,
        account_id: None,
        attention_at: Nullable(None),
        unread: false,
        unread_count: 0,
        asking: false,
        children: Vec::new(),
        parent_id: Nullable(None),
    }
}

/// Every visible Codex session, preferring the state database.
pub async fn list_codex_sessions(paths: &Paths) -> Vec<SessionSummary> {
    // Names the index that [`file_meta`] writes into; see
    // [`crate::db::index::current`] for why it is not an argument.
    crate::db::index::install(paths);
    let rows = on_state_db(paths, list_from_state_db).await.flatten();
    let mut summaries = match rows {
        Some(rows) => {
            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                // The rollout file is gone: there is nothing to open, so the
                // thread is not offered.
                let Ok(metadata) = tokio::fs::metadata(&row.rollout_path).await else {
                    continue;
                };
                let (summary, updated_ms) = row_to_summary(&row, metadata.len());
                out.push(with_codex_status(summary, updated_ms).await);
            }
            out
        }
        None => list_by_scanning(paths).await,
    };

    let titles = super::claude::custom_titles(paths, Source::Codex).await;
    if !titles.is_empty() {
        for summary in &mut summaries {
            if let Some(custom) = titles.get(&summary.id) {
                summary.title = custom.clone();
            }
        }
    }
    apply_ownership(paths, &mut summaries).await;
    summaries
}

/// Every thread id with a rollout file, or `None` when there is no corpus.
///
/// `None` and an empty set are different answers and the caller depends on it:
/// no `~/.codex/sessions` at all means codex has never run here, which is no
/// evidence that any particular thread is gone. See
/// [`super::codex_claims::prune`], the only caller.
pub(super) async fn rollout_ids(paths: &Paths) -> Option<HashSet<String>> {
    let dir = paths.codex_sessions();
    if !tokio::fs::try_exists(&dir).await.unwrap_or(false) {
        return None;
    }
    Some(
        collect_session_files(&dir)
            .await
            .iter()
            .filter_map(|(name, _)| id_from_file_name(name))
            .collect(),
    )
}

/// Promote this instance's own threads, and drop the ones a sibling made.
///
/// Every user-visible codex row in the crate is built by this file, so this is
/// the one place the three-way verdict of [`super::codex_claims`] has to be
/// applied — the chat list, the global feed and the session count all flow from
/// [`list_codex_sessions`], and the detail view from [`find_codex_session`],
/// which applies the same verdict to its single row so a hidden thread cannot be
/// opened by typing its URL.
///
/// Dropping rather than marking, for the rows another instance claimed: an
/// account-backed claude session lives in a root a sibling server cannot even
/// see, and this is what gives codex the same silence with one `~/.codex`.
///
/// One read of the registry and one of the claim directory per listing, not one
/// per row.
async fn apply_ownership(paths: &Paths, summaries: &mut Vec<SessionSummary>) {
    let owned = owned_sessions::read(paths).await;
    let claims = Claims::all(paths).await;
    let me = paths.instance();
    let local = super::ServerId::local(paths);
    summaries.retain_mut(|summary| {
        // Named before the verdict, and for every row that survives it: a thread
        // this instance recorded says the server that recorded it, and the
        // user's own threads say the server they are reachable through. A thread
        // a *sibling* instance claimed is dropped below rather than labelled —
        // the list has never shown those, and this slice does not change that.
        summary.server = owned.server_of(Source::Codex, &summary.id).unwrap_or(&local).to_string();
        match claims.verdict(me, &summary.id, owned.contains(Source::Codex, &summary.id)) {
            Verdict::Owned => {
                summary.scope = SessionScope::Owned;
                true
            }
            // The user's own `codex` threads: browsable from every instance,
            // drivable from none.
            Verdict::External => true,
            Verdict::Hidden => false,
        }
    });
}

/// The fallback path: every rollout file under `~/.codex/sessions`.
async fn list_by_scanning(paths: &Paths) -> Vec<SessionSummary> {
    let files = collect_session_files(&paths.codex_sessions()).await;
    let names = thread_names(paths).await;
    let mut out = Vec::with_capacity(files.len());
    for (name, path) in files {
        let Some(id) = id_from_file_name(&name) else { continue };
        let Ok(meta) = file_meta(&path, &id).await else { continue };
        out.push(with_codex_status(meta_to_summary(&meta, &names), meta.updated_ms).await);
    }
    out
}

/// Resolve one visible Codex session without listing the whole corpus.
pub async fn find_codex_session(paths: &Paths, id: &str) -> Option<SessionSummary> {
    if !crate::sessions::reference::is_path_segment_id(id) {
        return None;
    }
    // After the id check, not before: an id that cannot name a thread is not a
    // reason to open a database.
    crate::db::index::install(paths);

    let mut summary = match on_state_db(paths, {
        let id = id.to_string();
        move |db| query_thread_row(db, &id)
    })
    .await
    .flatten()
    {
        // Archived, a subagent thread, or unknown: stay hidden — unless it is a
        // thread a conversation delegated, which opens from its parent's tree.
        Some(None) => {
            let (row, parent_id) = on_state_db(paths, {
                let id = id.to_string();
                move |db| query_spawned_thread_row(db, &id)
            })
            .await
            .flatten()?;
            let metadata = tokio::fs::metadata(&row.rollout_path).await.ok()?;
            let (mut summary, updated_ms) = row_to_summary(&row, metadata.len());
            summary.parent_id = Nullable(Some(parent_id));
            with_codex_status(summary, updated_ms).await
        }
        Some(Some(row)) => {
            let metadata = tokio::fs::metadata(&row.rollout_path).await.ok()?;
            let (summary, updated_ms) = row_to_summary(&row, metadata.len());
            with_codex_status(summary, updated_ms).await
        }
        // The database is unavailable: fall back to scanning rollout files.
        None => {
            let files = collect_session_files(&paths.codex_sessions()).await;
            let (_, path) = files.into_iter().find(|(name, _)| id_from_file_name(name).as_deref() == Some(id))?;
            let meta = file_meta(&path, id).await.ok()?;
            let names = thread_names(paths).await;
            with_codex_status(meta_to_summary(&meta, &names), meta.updated_ms).await
        }
    };

    // The list overlays user-set titles; a detail that disagreed with the row the
    // user clicked would read as a different session.
    if let Some(custom) = super::claude::custom_titles(paths, Source::Codex).await.get(id) {
        summary.title = custom.clone();
    }
    // And the same for ownership: the composer's verdict is read off this row,
    // so it has to say what the list said — including saying nothing at all for
    // a thread another instance made, which the list does not show and this must
    // not resolve.
    let owned = owned_sessions::read(paths).await;
    summary.server = owned.server_of(Source::Codex, id).unwrap_or(&super::ServerId::local(paths)).to_string();
    match Claims::just(paths, id)
        .await
        .verdict(paths.instance(), id, owned.contains(Source::Codex, id))
    {
        Verdict::Owned => summary.scope = SessionScope::Owned,
        Verdict::External => {}
        Verdict::Hidden => return None,
    }
    Some(summary)
}

/// The model/effort a Codex session last ran with.
///
/// Port of `codexChoice` in the former TypeScript server's `session-choice.ts`.
pub async fn read_codex_cli_choice(file_path: &Path) -> Option<CliChoice> {
    let lines = read_tail_lines(file_path, CHOICE_TAIL_BYTES).await.ok()?;
    for line in lines.iter().rev() {
        let Some(entry) = try_parse(line) else { continue };
        if entry.get("type").and_then(Value::as_str) != Some("turn_context") {
            continue;
        }
        let Some(payload) = entry.get("payload").and_then(Value::as_object) else {
            continue;
        };
        let model = payload.get("model").and_then(Value::as_str).map(str::to_owned);
        let effort = payload.get("effort").and_then(Value::as_str).map(str::to_owned);
        if model.is_none() && effort.is_none() {
            continue;
        }
        return Some(CliChoice {
            model: Nullable(model),
            effort: Nullable(effort),
            at: Nullable(entry.get("timestamp").and_then(Value::as_str).map(str::to_owned)),
        });
    }
    None
}

/// Codex JSONL -> normalized transcript items.
///
/// Read as a stream rather than into memory: a long session's rollout runs to
/// hundreds of megabytes, and every line is independent — except for tool
/// calls, which [`CodexTools`] pairs with their outputs and, in code mode, with
/// the `item_completed` events that say what a cell actually ran.
pub async fn parse_codex_transcript(paths: &Paths, file_path: &Path) -> std::io::Result<Vec<TranscriptItem>> {
    let mut lines = LossyLines::open(file_path).await?;
    let mut items: Vec<TranscriptItem> = Vec::new();
    let mut tools = CodexTools::new();
    let mut cwd = None;
    // The model the turn being read runs on. Codex names it once, in the
    // `turn_context` that opens each turn, so every item below one belongs to it
    // until the next one says otherwise — which is the only record of a silent
    // downgrade mid-session. `None` before the first one: the rollout has not
    // said yet, and guessing would attribute a turn to the wrong model.
    let mut model: Option<String> = None;

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let Some(entry) = try_parse(&line) else { continue };
        if entry.get("type").and_then(Value::as_str) == Some("session_meta") {
            cwd = entry.get("payload").and_then(|p| p.get("cwd")).and_then(Value::as_str).map(str::to_owned);
            continue;
        }
        if entry.get("type").and_then(Value::as_str) == Some("turn_context") {
            model = entry
                .get("payload")
                .and_then(Value::as_object)
                .and_then(|payload| payload.get("model"))
                .and_then(Value::as_str)
                .filter(|model| !model.trim().is_empty())
                .map(str::to_owned);
            continue;
        }
        let Some(payload) = entry.get("payload").and_then(Value::as_object) else {
            continue;
        };
        let timestamp = entry.get("timestamp").and_then(Value::as_str);
        let model = model.as_deref();
        let context = Context {
            home: &paths.home,
            cwd: cwd.as_deref(),
        };

        match entry.get("type").and_then(Value::as_str) {
            // Every other `event_msg` echoes a response item, or is bookkeeping.
            Some("event_msg") if payload.get("type").and_then(Value::as_str) == Some("item_completed") => {
                tools.item_completed(&mut items, payload, timestamp, model, context);
            }
            Some("response_item") => match payload.get("type").and_then(Value::as_str) {
                Some("message") => push_message(&mut items, payload, timestamp, model),
                Some("reasoning") => push_reasoning(&mut items, payload, timestamp, model),
                Some("custom_tool_call") | Some("function_call") | Some("local_shell_call") => {
                    tools.call(&mut items, payload, timestamp, model);
                }
                Some("function_call_output") | Some("custom_tool_call_output") => tools.output(&mut items, payload, timestamp, context),
                Some("web_search_call") => tools.web_search_call(&mut items, payload, timestamp, model),
                _ => {}
            },
            _ => {}
        }
    }
    super::mentions::annotate_mentioned_files(&paths.home, &mut items, cwd.as_deref()).await;
    Ok(items)
}

/// Attribute an agent-produced item to the model of the turn it came from.
fn by_model(mut item: TranscriptItem, model: Option<&str>) -> TranscriptItem {
    item.model = model.map(str::to_owned);
    item
}

/// A user or assistant message, one item per non-empty text part.
fn push_message(items: &mut Vec<TranscriptItem>, payload: &serde_json::Map<String, Value>, timestamp: Option<&str>, model: Option<&str>) {
    // system/developer messages carry injected instructions; skip them.
    let role = match payload.get("role").and_then(Value::as_str) {
        Some("user") => Role::User,
        Some("assistant") => Role::Assistant,
        _ => return,
    };
    let parts = payload.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
    for part in parts {
        let Some(text) = part.get("text").and_then(Value::as_str) else {
            continue;
        };
        if text.trim().is_empty() || is_wrapper_text(text) {
            continue;
        }
        if role == Role::Assistant {
            items.push(by_model(make_item(role, ItemKind::Text, text, timestamp), model));
            continue;
        }
        let split = parse_attachment_suffix(text);
        let mut item = make_item(role, ItemKind::Text, &split.text, timestamp);
        if !split.images.is_empty() {
            item.images = Some(split.images);
        }
        if !split.files.is_empty() {
            item.files = Some(split.files);
        }
        items.push(item);
    }
}

/// Extended thinking, which Codex re-emits cumulatively.
fn push_reasoning(items: &mut Vec<TranscriptItem>, payload: &serde_json::Map<String, Value>, timestamp: Option<&str>, model: Option<&str>) {
    let summaries = payload.get("summary").and_then(Value::as_array).cloned().unwrap_or_default();
    let text = summaries
        .iter()
        .filter_map(|entry| entry.get("text").and_then(Value::as_str))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    if text.is_empty() {
        return;
    }

    // Each reasoning entry extends the last, so keep only the longest variant
    // instead of stacking near-duplicates.
    if let Some(previous) = items.last() {
        if previous.kind == ItemKind::Thinking && (text.starts_with(&previous.text) || previous.text.starts_with(text)) {
            if text.len() > previous.text.len() {
                let index = items.len() - 1;
                items[index] = by_model(make_item(Role::Assistant, ItemKind::Thinking, text, timestamp), model);
            }
            return;
        }
    }
    items.push(by_model(make_item(Role::Assistant, ItemKind::Thinking, text, timestamp), model));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn rollout(lines: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout-2026-05-09T16-39-49-11111111-2222-3333-4444-555555555555.jsonl");
        let mut file = std::fs::File::create(&path).expect("create");
        for line in lines {
            writeln!(file, "{line}").expect("write");
        }
        (dir, path)
    }

    #[test]
    fn only_a_rollout_file_name_yields_an_id() {
        assert_eq!(
            id_from_file_name("rollout-2026-05-09T16-39-49-11111111-2222-3333-4444-555555555555.jsonl").as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
        assert!(id_from_file_name("rollout-2026-05-09.jsonl").is_none());
        assert!(id_from_file_name("11111111-2222-3333-4444-555555555555.jsonl").is_none());
    }

    #[tokio::test]
    async fn the_head_gives_a_cwd_and_the_first_real_user_message() {
        let (_dir, path) = rollout(&[
            r#"{"type":"session_meta","payload":{"cwd":"/w","id":"x"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>noise</environment_context>"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"real question"}]}}"#,
        ]);
        let meta = file_meta(&path, "abc").await.expect("meta");
        assert_eq!(meta.cwd.as_deref(), Some("/w"));
        assert_eq!(
            meta.title_fallback.as_deref(),
            Some("real question"),
            "a wrapper preamble is never the title"
        );
    }

    #[tokio::test]
    async fn status_comes_from_the_last_meaningful_tail_entry() {
        let (_dir, waiting) = rollout(&[r#"{"type":"event_msg","payload":{"type":"agent_message"}}"#]);
        assert_eq!(codex_status(&waiting, now_ms()).await, SessionStatus::Waiting);

        let (_dir2, running) = rollout(&[
            r#"{"type":"event_msg","payload":{"type":"agent_message"}}"#,
            r#"{"type":"event_msg","payload":{"type":"user_message"}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count"}}"#,
        ]);
        assert_eq!(
            codex_status(&running, now_ms()).await,
            SessionStatus::Running,
            "token_count carries no signal, so the user message decides"
        );

        let (_dir3, tool) = rollout(&[r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command"}}"#]);
        assert_eq!(codex_status(&tool, now_ms()).await, SessionStatus::Running);

        let (_dir4, old) = rollout(&[r#"{"type":"event_msg","payload":{"type":"user_message"}}"#]);
        assert_eq!(
            codex_status(&old, now_ms() - ACTIVE_WINDOW_MS - 1).await,
            SessionStatus::Idle,
            "an untouched session is idle whatever its tail says"
        );
    }

    /// Codex has nothing to ask with — approvals are off and every interactive
    /// request its app server makes is refused — so a finished turn is a wait
    /// and never a question.
    #[tokio::test]
    async fn a_finished_codex_turn_waits_but_never_asks() {
        let (_dir, path) = rollout(&[r#"{"type":"event_msg","payload":{"type":"agent_message"}}"#]);
        let meta = FileMeta {
            id: "t".into(),
            file_path: path,
            cwd: None,
            title_fallback: None,
            updated_ms: now_ms(),
            updated_at: String::new(),
            created_at: String::new(),
            size_bytes: 0,
        };
        let summary = with_codex_status(meta_to_summary(&meta, &HashMap::new()), meta.updated_ms).await;
        assert_eq!((summary.status, summary.asking), (SessionStatus::Waiting, false));
    }

    #[tokio::test]
    async fn a_transcript_normalises_messages_reasoning_and_tools() {
        let (dir, path) = rollout(&[
            r#"{"type":"response_item","timestamp":"2026-05-09T16:39:50.000Z","payload":{"type":"message","role":"system","content":[{"text":"instructions"}]}}"#,
            r#"{"type":"response_item","timestamp":"2026-05-09T16:39:51.000Z","payload":{"type":"message","role":"user","content":[{"text":"do it"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"think"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"think harder"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"ls\"}"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call_output","output":"a.txt\nb.txt"}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count"}}"#,
        ]);
        let items = parse_codex_transcript(&Paths::new(dir.path().to_path_buf()), &path).await.expect("parse");

        assert_eq!(items.len(), 4, "the system message and the event_msg are not transcript items");
        assert_eq!(items[0].role, Role::User);
        assert_eq!(items[0].text, "do it");
        assert_eq!(items[0].timestamp.as_ref().map(String::as_str), Some("2026-05-09T16:39:51.000Z"));
        assert_eq!(items[1].kind, ItemKind::Thinking);
        assert_eq!(items[1].text, "think harder", "cumulative reasoning replaces its own prefix");
        assert_eq!(items[2].kind, ItemKind::ToolUse);
        assert_eq!(items[2].text, "ls", "a codex command has no description, so the row is the command");
        assert_eq!(items[3].kind, ItemKind::ToolResult);
        assert_eq!(items[3].role, Role::User);
        assert_eq!(items[3].text, "a.txt");
    }

    #[tokio::test]
    async fn an_attachment_suffix_becomes_files_on_the_item() {
        let (dir, path) = rollout(&[
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"look at this\n\n[1 attached file(s) — view with the Read tool:]\n- /tmp/a.png\n"}]}}"#,
        ]);
        let items = parse_codex_transcript(&Paths::new(dir.path().to_path_buf()), &path).await.expect("parse");
        assert_eq!(items[0].text, "look at this");
        assert_eq!(items[0].images.as_deref(), Some(["/tmp/a.png".to_string()].as_slice()));
    }

    #[tokio::test]
    async fn the_cli_choice_comes_from_the_newest_turn_context() {
        let (_dir, path) = rollout(&[
            r#"{"type":"turn_context","timestamp":"2026-05-09T16:00:00.000Z","payload":{"model":"gpt-5.4","effort":"low"}}"#,
            r#"{"type":"turn_context","timestamp":"2026-05-09T17:00:00.000Z","payload":{"model":"gpt-5.5","effort":"high"}}"#,
        ]);
        let choice = read_codex_cli_choice(&path).await.expect("choice");
        assert_eq!(choice.model.as_ref().map(String::as_str), Some("gpt-5.5"));
        assert_eq!(choice.effort.as_ref().map(String::as_str), Some("high"));
        assert_eq!(choice.at.as_ref().map(String::as_str), Some("2026-05-09T17:00:00.000Z"));
    }

    /// Codex names the model once per turn, in the `turn_context` that opens it,
    /// so an item takes the newest one above it — and a downgrade is visible only
    /// as the next `turn_context` saying something else.
    #[tokio::test]
    async fn items_take_the_model_of_the_newest_turn_context_above_them() {
        let (dir, path) = rollout(&[
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"before any turn"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"text":"unattributed"}]}}"#,
            r#"{"type":"turn_context","payload":{"model":"gpt-5.5","effort":"high"}}"#,
            r#"{"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"think"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"text":"from the big one"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"ls\"}"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call_output","output":"a.txt"}}"#,
            r#"{"type":"turn_context","payload":{"model":"gpt-5.5-codex-mini","effort":"low"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"again"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"text":"from the small one"}]}}"#,
        ]);
        let items = parse_codex_transcript(&Paths::new(dir.path().to_path_buf()), &path).await.expect("parse");

        assert_eq!(items.len(), 8, "a turn_context is context, not an item");
        assert_eq!(items[0].model, None, "nothing the user typed is attributed to a model");
        assert_eq!(items[1].model, None, "before the first turn_context the rollout has not said");
        assert_eq!(items[2].kind, ItemKind::Thinking);
        for item in &items[2..5] {
            assert_eq!(item.model.as_deref(), Some("gpt-5.5"), "thinking, text and tool calls alike");
        }
        assert_eq!(items[4].kind, ItemKind::ToolUse);
        assert_eq!(items[5].model, None, "a tool result is not the agent's own item");
        assert_eq!(items[6].model, None);
        assert_eq!(
            items[7].model.as_deref(),
            Some("gpt-5.5-codex-mini"),
            "the later turn_context relabels everything after it"
        );
    }

    #[tokio::test]
    async fn a_traversing_id_never_resolves() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        assert!(find_codex_session(&paths, "../../etc/passwd").await.is_none());
        assert!(find_codex_session(&paths, "").await.is_none());
    }

    /// A thread the user started in their own terminal is nobody's here — and
    /// it is still reached through this server, which is what the row says.
    ///
    /// The other half of `claude::tests::a_discovered_session_is_reachable_...`,
    /// on the CLI where discovery has no root to distinguish "mine" by.
    #[tokio::test]
    async fn a_discovered_thread_names_the_server_it_is_reachable_through() {
        let _guard = crate::db::index::exclusive().await;
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(home.path().to_path_buf(), "alice");

        // No `state_5.sqlite`, so discovery falls back to scanning rollouts —
        // which is the path a fresh machine takes anyway.
        let id = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";
        let day = paths.codex_sessions().join("2026").join("09").join("21");
        std::fs::create_dir_all(&day).expect("rollout dir");
        std::fs::write(
            day.join(format!("rollout-2026-09-21T10-00-00-{id}.jsonl")),
            "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/w\",\"id\":\"x\"}}\n",
        )
        .expect("rollout");

        let listed = list_codex_sessions(&paths).await;
        let row = listed.iter().find(|row| row.id == id).expect("the thread is listed");
        assert_eq!(row.scope, SessionScope::External, "the user's own thread is not this server's to drive");
        assert_eq!(row.server, "alice", "but this server is where it can be reached");
        let found = find_codex_session(&paths, id).await.expect("the same row, one at a time");
        assert_eq!((found.scope, found.server.as_str()), (SessionScope::External, "alice"));

        // Recorded as this instance's own: the value now comes off the row.
        owned_sessions::record(&paths, Source::Codex, id).await.expect("record");
        let mine = find_codex_session(&paths, id).await.expect("the row");
        assert_eq!(mine.scope, SessionScope::Owned);
        assert_eq!(mine.server, "alice");
    }

    /// A rollout head is up to 512 KiB, so this is the read most worth not
    /// repeating after a restart — and the one whose payload has to survive a
    /// serialise/deserialise round trip that nothing else in this file does.
    #[tokio::test]
    async fn a_rollout_read_once_is_not_read_again_after_a_restart() {
        let _guard = crate::db::index::exclusive().await;
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        crate::db::index::install(&paths);

        let id = "durable-index-fixture";
        let (_dir, path) = rollout(&[
            r#"{"type":"session_meta","payload":{"cwd":"/w","id":"x"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"from the rollout"}]}}"#,
        ]);
        let first = file_meta(&path, id).await.expect("meta");
        assert_eq!(first.title_fallback.as_deref(), Some("from the rollout"));

        META_CACHE.lock().expect("codex meta cache poisoned").clear();
        let index = crate::db::index::open(&paths);
        let metadata = std::fs::metadata(&path).expect("stat");
        let planted = FileMeta {
            title_fallback: Some("read out of the index".to_string()),
            ..first.clone()
        };
        index.remember_summary(Source::Codex, id, crate::db::index::Watermark::of(&path, &metadata), &planted);

        let after = file_meta(&path, id).await.expect("meta");
        assert_eq!(
            after.title_fallback.as_deref(),
            Some("read out of the index"),
            "the head is not parsed again for a rollout that has not moved"
        );
        assert_eq!(after.cwd.as_deref(), Some("/w"), "and the rest of the row round-trips intact");
    }
}
