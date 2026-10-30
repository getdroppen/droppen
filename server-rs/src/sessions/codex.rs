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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use futures_util::future::BoxFuture;
use serde_json::Value;

use droppen_sessions::children::{ReadMarksExt, SessionChild, SessionChildKind, SessionChildStatus, SpawnOrderExt};
use super::codex_claims::{Claims, Verdict};
use super::codex_tools::{CodexTools, Context};
use droppen_instance::paths::Paths;
use droppen_base::time::{iso8601, mtime_ms, now_ms};
use droppen_sessions::context::ContextUsage;
use droppen_sessions::mentions::FileCitation;
use droppen_sessions::sources::{TokenUsage, UsageFile, UsageFold};
use droppen_sessions::util::{
    date_of, is_jsonl, make_item, num, read_head_lines, read_tail_lines, title_line, try_parse, FileCache, ItemKind, LossyLines, Role,
    TranscriptItem,
};
use droppen_sessions::{owned_sessions, CliChoice, SessionActivity, SessionScope, SessionStatus, SessionSummary, Source};
use droppen_http::openapi_ext::Nullable;

/// The files in `~/.codex` that only the session list and the model list read:
/// Codex's thread index, its per-thread name index and its model cache.
/// `~/.codex` itself and its `sessions/` are shared ([`Paths::codex_dir`]).
#[derive(Debug, Clone)]
pub struct CodexHome(PathBuf);

impl CodexHome {
    /// `~/.codex` of the home `paths` names.
    pub fn of(paths: &Paths) -> Self {
        Self(paths.codex_dir())
    }

    /// Codex's thread index database.
    pub fn state_db(&self) -> PathBuf {
        self.0.join("state_5.sqlite")
    }

    /// Codex's optional per-thread name index.
    pub fn session_index(&self) -> PathBuf {
        self.0.join("session_index.jsonl")
    }

    /// Codex's cache of the models its backend offers.
    pub fn models_cache(&self) -> PathBuf {
        self.0.join("models_cache.json")
    }
}

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
/// [`droppen_sessions::index`] stores: what survives a restart is this struct, not the
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
/// Behind it is [`droppen_sessions::index`], which remembers the same thing across a
/// restart.
static META_CACHE: LazyLock<FileCache<FileMeta>> = LazyLock::new(|| FileCache::new(CACHE_MAX));

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
    let mark = droppen_sessions::index::Watermark::of(file_path, &metadata);
    if let Some(meta) = META_CACHE.get(file_path, mtime, metadata.len()) {
        // Size too, so this tier and the durable one agree on what "unchanged"
        // means.
        return Ok(meta);
    }

    // What a previous boot read out of this exact file. A rollout's head can be
    // half a megabyte — the `session_meta` line alone carries the base
    // instructions — so this is the read worth not repeating.
    let index = droppen_sessions::index::current();
    if let Some(meta) = index.as_ref().and_then(|index| index.summary::<FileMeta>(Source::Codex, id, mark)) {
        META_CACHE.put(file_path, mtime, meta.size_bytes, meta.clone());
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
    META_CACHE.put(file_path, mtime, meta.size_bytes, meta.clone());
    if let Some(index) = index {
        index.remember_summary(Source::Codex, id, mark, &meta);
    }
    Ok(meta)
}

impl CodexHome {
    /// Thread names from the optional `session_index.jsonl`.
    async fn thread_names(&self) -> HashMap<String, String> {
        let mut names = HashMap::new();
        // The index is optional; without it sessions fall back to their first message.
        let Ok(raw) = tokio::fs::read_to_string(self.session_index()).await else {
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
struct SpawnedThread {
    id: String,
    parent_id: String,
    /// The name Codex gave it (`agent_nickname`), or its title.
    name: String,
    /// What it was spawned as (`agent_role`), when Codex says.
    role: Option<String>,
    rollout_path: String,
    updated_ms: i64,
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
async fn spawned_threads(paths: &Paths) -> Vec<SpawnedThread> {
    CodexHome::of(paths).on_state_db(spawned_threads_from_state_db).await.flatten().unwrap_or_default()
}

/// How a subagent thread is doing: whether it is still working, and whether
/// its last turn ended in an error or was aborted.
async fn spawned_thread_state(thread: &SpawnedThread) -> (bool, bool) {
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

/// Codex children of `parent`, nested; `depth` guards against a cycle in a
/// damaged edge table.
async fn codex_children(
    parent: &str,
    by_parent: &HashMap<&str, Vec<&SpawnedThread>>,
    marks: &BTreeMap<String, String>,
    parent_read: Option<&str>,
    depth: usize,
) -> Vec<SessionChild> {
    let Some(threads) = by_parent.get(parent).filter(|_| depth < 8) else {
        return Vec::new();
    };
    let mut children = Vec::with_capacity(threads.len());
    for thread in threads {
        let (working, failed) = spawned_thread_state(thread).await;
        let status = match (working, failed) {
            (true, _) => SessionChildStatus::Working,
            (false, true) => SessionChildStatus::Failed,
            (false, false) => SessionChildStatus::Done,
        };
        let updated_at = iso8601(SystemTime::UNIX_EPOCH + Duration::from_millis(thread.updated_ms.max(0) as u64));
        let own = marks.read_at(Source::Codex, &thread.id);
        let read = own.max(parent_read);
        let grandchildren = Box::pin(codex_children(&thread.id, by_parent, marks, read, depth + 1)).await;
        children.push(SessionChild {
            id: thread.id.clone(),
            kind: SessionChildKind::Thread,
            name: thread.name.clone(),
            role: Nullable(thread.role.clone()),
            status,
            unread: status.is_unread(&updated_at, own, parent_read),
            updated_at,
            children: grandchildren,
        });
    }
    children.sort_by_spawn();
    children
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

impl CodexHome {
    /// Run a state-database query off the async runtime's worker threads.
    ///
    /// SQLite is a blocking API, and a cold page cache on a large index is long
    /// enough to matter to every other request in flight.
    async fn on_state_db<T: Send + 'static>(&self, query: impl FnOnce(&Path) -> T + Send + 'static) -> Option<T> {
        let db = self.state_db();
        tokio::task::spawn_blocking(move || query(&db)).await.ok()
    }
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
        acp_agent: None,
        account_id: None,
        // All filled in by `droppen_sessions::attention::apply`, once per changed file.
        attention_at: Nullable(None),
        unread: false,
        unread_count: 0,
        // Never set for codex: it runs with approvals off and every interactive
        // request the app server makes is refused (`provider::app_server`), so a
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
        acp_agent: None,
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
    // [`droppen_sessions::index::current`] for why it is not an argument.
    droppen_sessions::index::install(paths);
    let rows = CodexHome::of(paths).on_state_db(list_from_state_db).await.flatten();
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

    summaries.finish(paths, Claims::all(paths).await).await;
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

/// The overlays a listing and a single-row lookup both apply to Codex rows.
trait FinishExt {
    /// Apply the user's own title and the ownership verdict.
    ///
    /// `claims` is every claim on the machine for a listing, and just this
    /// thread's for a one-row lookup. Both paths drop a `Hidden` row rather than
    /// labelling it, so a thread another instance made cannot be opened by its id
    /// either.
    async fn finish(&mut self, paths: &Paths, claims: Claims);
}

impl FinishExt for Vec<SessionSummary> {
    async fn finish(&mut self, paths: &Paths, claims: Claims) {
        let titles = droppen_sessions::custom_titles(paths, Source::Codex).await;
        if !titles.is_empty() {
            for summary in self.iter_mut() {
                if let Some(custom) = titles.get(&summary.id) {
                    summary.title = custom.clone();
                }
            }
        }
        apply_ownership(paths, self, claims).await;
    }
}

/// Promote this instance's own threads, and drop the ones a sibling made.
///
/// Every user-visible codex row in the crate is built by this file, so this is
/// the one place the three-way verdict of [`super::codex_claims`] has to be
/// applied — the chat list, the global feed and the session count all flow from
/// [`list_codex_sessions`], and the detail view from [`find_codex_session`],
/// which runs [`FinishExt::finish`] on its single row so a hidden thread cannot
/// be opened by typing its URL.
///
/// Dropping rather than marking, for the rows another instance claimed: an
/// account-backed claude session lives in a root a sibling server cannot even
/// see, and this is what gives codex the same silence with one `~/.codex`.
///
/// One read of the registry per pass, not one per row.
async fn apply_ownership(paths: &Paths, summaries: &mut Vec<SessionSummary>, claims: Claims) {
    let owned = owned_sessions::read(paths).await;
    let me = paths.instance();
    let local = droppen_instance::identity::ServerId::local(paths);
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
    let names = CodexHome::of(paths).thread_names().await;
    let mut out = Vec::with_capacity(files.len());
    for (name, path) in files {
        let Some(id) = id_from_file_name(&name) else { continue };
        let Ok(meta) = file_meta(&path, &id).await else { continue };
        out.push(with_codex_status(meta_to_summary(&meta, &names), meta.updated_ms).await);
    }
    out
}

/// Codex's transcripts, for [`droppen_sessions::TranscriptSources`]: the rollouts under
/// `~/.codex`, found through its state database or by walking them.
pub struct CodexSessions;

impl CodexSessions {
    /// Record that this instance made thread `id`: in its ownership registry,
    /// then as a claim in the shared directory ([`super::codex_claims::claim`],
    /// this reader's [`on_owned`](droppen_sessions::Transcripts::on_owned)).
    /// [`OwnedSessions::record`](owned_sessions::OwnedSessions::record) writes
    /// every row and runs the step after it, so the two cannot drift.
    ///
    /// The registry's outcome is the answer. The claim is a separate,
    /// best-effort copy of what was just recorded, and it is re-attempted on a
    /// second `record_owned` of the same id precisely so a claim that failed to
    /// land once is not permanent.
    pub async fn record_owned(&self, paths: &Paths, id: &str) -> std::io::Result<()> {
        owned_sessions::OwnedSessions::record(paths, self, id).await
    }
}

impl droppen_sessions::Transcripts for CodexSessions {
    fn source(&self) -> Source {
        Source::Codex
    }

    /// False: codex keeps every thread in the user's own `~/.codex`, so there
    /// is no root of the server's own to look in. The threads this server
    /// started are in its ownership records, which the guide reads first.
    fn has_own_sessions<'a>(&'a self, _paths: &'a Paths) -> BoxFuture<'a, bool> {
        Box::pin(async { false })
    }

    fn list<'a>(&'a self, paths: &'a Paths, bindings: &'a BTreeMap<String, String>) -> BoxFuture<'a, Vec<SessionSummary>> {
        Box::pin(async move {
            let mut sessions = list_codex_sessions(paths).await;
            // The claude discovery applies the bindings itself, inside the root walk it
            // has to do anyway; codex has no such walk, so its rows are matched up here.
            // Either way the field means the same thing and is filled from the same
            // table — a client reading `accountId` must not have to ask which CLI wrote
            // the row.
            for session in &mut sessions {
                session.account_id = bindings.get(&session.id).cloned();
            }
            sessions
        })
    }

    // Also what `find_file` reads the path off: a database row, and a status
    // read of one 64 KiB tail for a recent rollout only, cheap enough not to
    // need a lookup of its own.
    fn find<'a>(&'a self, paths: &'a Paths, id: &'a str) -> BoxFuture<'a, Option<SessionSummary>> {
        Box::pin(find_codex_session(paths, id))
    }

    // A codex thread lives under `~/.codex`, in no Claude root.
    fn find_root<'a>(&'a self, _paths: &'a Paths, _id: &'a str) -> BoxFuture<'a, Option<droppen_instance::paths::ClaudeRoot>> {
        Box::pin(std::future::ready(None))
    }

    fn parse<'a>(&'a self, paths: &'a Paths, file_path: &'a Path) -> BoxFuture<'a, std::io::Result<Vec<TranscriptItem>>> {
        Box::pin(parse_codex_transcript(paths, file_path))
    }

    fn cli_choice<'a>(&'a self, file_path: &'a Path) -> BoxFuture<'a, Option<CliChoice>> {
        Box::pin(read_codex_cli_choice(file_path))
    }

    // A killed Codex process may never append `turn_aborted`, leaving a recent
    // `task_started` as the rollout's last word.
    fn settles_reaped_runs(&self) -> bool {
        true
    }

    fn external_reason(&self) -> String {
        "this codex session was started elsewhere; continue it where it was started".to_string()
    }

    fn live_block(&self, status: SessionStatus) -> Option<String> {
        (status == SessionStatus::Running).then(|| "session is currently running".into())
    }

    fn foreign_sessions(&self) -> bool {
        true
    }

    // One `~/.codex` for every instance: the claim is what tells the siblings
    // the thread is this one's.
    fn on_owned<'a>(&'a self, paths: &'a Paths, id: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(super::codex_claims::claim(paths, id))
    }

    fn watch_dirs<'a>(&'a self, paths: &'a Paths) -> BoxFuture<'a, Vec<PathBuf>> {
        Box::pin(std::future::ready(vec![paths.codex_sessions()]))
    }

    fn context<'a>(&'a self, file_path: &'a Path) -> BoxFuture<'a, Option<droppen_sessions::context::ContextUsage>> {
        Box::pin(read_codex_context(file_path))
    }

    // Each non-empty text part of an assistant `message` payload, minus the
    // injected wrappers (`<environment_context>`, `AGENTS.md`, …), which are
    // instructions rather than speech. `reasoning` and tool calls are not
    // messages, and an `event_msg` is an echo of a `response_item` already
    // read. Codex has no sidechain of its own: a subagent thread is a separate
    // rollout file, hidden from the list by discovery.
    fn spoken_texts(&self, entry: &serde_json::Map<String, Value>) -> Vec<String> {
        let keep = |text: &str| {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        };
        if entry.get("type").and_then(Value::as_str) != Some("response_item") {
            return Vec::new();
        }
        let Some(payload) = entry.get("payload").and_then(Value::as_object) else {
            return Vec::new();
        };
        if payload.get("type").and_then(Value::as_str) != Some("message") || payload.get("role").and_then(Value::as_str) != Some("assistant") {
            return Vec::new();
        }
        payload
            .get("content")
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .filter(|text| !is_wrapper_text(text))
                    .filter_map(keep)
                    .collect()
            })
            .unwrap_or_default()
    }

    // Codex nests rollouts under `sessions/YYYY/MM/DD`, so the tree is walked.
    //
    // An explicit stack rather than recursion: a recursive `async fn` needs its
    // future boxed at every level, which buys nothing here.
    fn usage_files<'a>(&'a self, paths: &'a Paths, cutoff_ms: f64) -> BoxFuture<'a, Vec<UsageFile>> {
        Box::pin(async move {
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
                        if let Some(reference) = UsageFile::stat(path, cutoff_ms).await {
                            out.push(reference);
                        }
                    }
                }
            }
            out
        })
    }

    fn usage_fold(&self) -> Box<dyn UsageFold> {
        Box::new(CodexUsage::default())
    }

    // Codex runs a subagent as a thread of its own and records the edge in
    // its state database (`thread_spawn_edges`). The thread is a session and
    // opens by its id; the list keeps it out of the top level and shows it
    // here instead. The edges are one table for every session, so they are
    // read once for all of them.
    fn children<'a>(&'a self, paths: &'a Paths, sessions: Vec<&'a mut SessionSummary>, marks: &'a BTreeMap<String, String>) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let codex_threads = spawned_threads(paths).await;
            let mut threads_by_parent: HashMap<&str, Vec<&SpawnedThread>> = HashMap::new();
            for thread in &codex_threads {
                threads_by_parent.entry(thread.parent_id.as_str()).or_default().push(thread);
            }
            for session in sessions {
                session.children = codex_children(&session.id, &threads_by_parent, marks, marks.read_at(Source::Codex, &session.id), 0).await;
            }
        })
    }
}

/// Codex emits *cumulative* `token_count` events; the per-turn delta lives in
/// `last_token_usage`, and the active model comes from the preceding
/// `turn_context`. Events can repeat, so only a strict increase of the running
/// total is counted.
#[derive(Debug)]
struct CodexUsage {
    model: String,
    prev_total: f64,
}

impl Default for CodexUsage {
    fn default() -> Self {
        Self {
            model: "codex".to_string(),
            prev_total: 0.0,
        }
    }
}

impl UsageFold for CodexUsage {
    fn fold(&mut self, entry: &serde_json::Map<String, Value>) -> Option<TokenUsage> {
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
            return None;
        }
        if entry_type != Some("event_msg") {
            return None;
        }
        let Some(payload) = entry.get("payload").and_then(Value::as_object) else {
            return None;
        };
        if payload.get("type").and_then(Value::as_str) != Some("token_count") {
            return None;
        }
        let Some(info) = payload.get("info").and_then(Value::as_object) else {
            return None;
        };
        let total = info
            .get("total_token_usage")
            .and_then(Value::as_object)
            .map(|usage| num(usage.get("total_tokens")))
            .unwrap_or(0.0);
        if total <= self.prev_total {
            return None; // duplicate / non-progressing report
        }
        self.prev_total = total;
        let Some(date) = date_of(entry) else { return None };
        let Some(last) = info.get("last_token_usage").and_then(Value::as_object) else {
            return None;
        };
        let cache_read = num(last.get("cached_input_tokens"));
        Some(TokenUsage {
            date,
            model: self.model.clone(),
            // `input_tokens` already includes whatever came out of the cache.
            input: (num(last.get("input_tokens")) - cache_read).max(0.0),
            output: num(last.get("output_tokens")),
            cache_read,
            cache_write: 0.0,
        })
    }
}

/// Resolve one visible Codex session without listing the whole corpus.
pub async fn find_codex_session(paths: &Paths, id: &str) -> Option<SessionSummary> {
    if !droppen_sessions::reference::is_path_segment_id(id) {
        return None;
    }
    // After the id check, not before: an id that cannot name a thread is not a
    // reason to open a database.
    droppen_sessions::index::install(paths);
    let home = CodexHome::of(paths);

    let summary = match home.on_state_db({
        let id = id.to_string();
        move |db| query_thread_row(db, &id)
    })
    .await
    .flatten()
    {
        // Archived, a subagent thread, or unknown: stay hidden — unless it is a
        // thread a conversation delegated, which opens from its parent's tree.
        Some(None) => {
            let (row, parent_id) = home.on_state_db({
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
            let names = home.thread_names().await;
            with_codex_status(meta_to_summary(&meta, &names), meta.updated_ms).await
        }
    };

    // The list overlays user-set titles and ownership; a detail that disagreed
    // with the row the user clicked would read as a different session. A thread
    // another instance made is dropped by the ownership verdict, so it does not
    // resolve here either.
    let mut summaries = vec![summary];
    summaries.finish(paths, Claims::just(paths, id).await).await;
    summaries.pop()
}

/// The model/effort a Codex session last ran with.
///
/// Port of `codexChoice` in the former TypeScript server's `session-choice.ts`.
pub async fn read_codex_cli_choice(file_path: &Path) -> Option<CliChoice> {
    CliChoice::read_tail(file_path, CHOICE_TAIL_BYTES, |entry| {
        if entry.get("type").and_then(Value::as_str) != Some("turn_context") {
            return None;
        }
        let payload = entry.get("payload").and_then(Value::as_object)?;
        let model = payload.get("model").and_then(Value::as_str).map(str::to_owned);
        let effort = payload.get("effort").and_then(Value::as_str).map(str::to_owned);
        (model.is_some() || effort.is_some()).then_some((model, effort))
    })
    .await
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
    droppen_sessions::mentions::annotate_mentioned_files(&paths.home, &mut items, cwd.as_deref(), file_citations).await;
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
        items.push(TranscriptItem::user_text(text, timestamp));
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

/// Codex's occupancy: the newest `token_count` event's `last_token_usage`.
///
/// Deliberately *not* `total_token_usage`, which is cumulative over the session —
/// a long thread reports tens of millions against a 258k window, which is spend,
/// not occupancy. `last_token_usage.input_tokens` is the per-turn figure and
/// already includes whatever was replayed out of the cache, matching what the
/// Claude arm sums by hand.
async fn read_codex_context(file_path: &Path) -> Option<ContextUsage> {
    let lines = read_tail_lines(file_path, droppen_sessions::context::TAIL_BYTES).await.ok()?;
    for line in lines.iter().rev() {
        let Some(entry) = try_parse(line) else { continue };
        if entry.get("type").and_then(Value::as_str) != Some("event_msg") {
            continue;
        }
        let Some(payload) = entry.get("payload").and_then(Value::as_object) else {
            continue;
        };
        if payload.get("type").and_then(Value::as_str) != Some("token_count") {
            continue;
        }
        let Some(info) = payload.get("info").and_then(Value::as_object) else {
            continue;
        };
        let window = info.get("model_context_window").and_then(Value::as_u64).filter(|w| *w > 0);
        let tokens = info
            .get("last_token_usage")
            .and_then(Value::as_object)
            .and_then(|last| last.get("input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if tokens > 0 {
            // No model is recorded here on purpose: codex states its own window
            // on the same event, so nothing downstream needs to infer one.
            return Some(ContextUsage { tokens, window, model: None });
        }
    }
    None
}

/// Codex's quoted file directive is structured data, not a prose token. In
/// particular, spaces, Unicode and parentheses are all part of the path.
pub fn file_citations(text: &str) -> Vec<FileCitation> {
    static CITATION: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r#":codex-file-citation\{((?:[^"{}]|"(?:\\.|[^"\\])*")*)\}"#).expect("valid file-citation pattern"));
    static ATTRIBUTE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r#"^([A-Za-z][A-Za-z0-9_-]*)\s*=\s*("(?:\\.|[^"\\])*")"#).expect("valid citation-attribute pattern"));
    CITATION
        .captures_iter(text)
        .filter_map(|citation| {
            let mut attrs = citation[1].trim();
            let mut path = None;
            while !attrs.is_empty() {
                let attribute = ATTRIBUTE.captures(attrs)?;
                let value: String = serde_json::from_str(&attribute[2]).ok()?;
                if &attribute[1] == "path" && path.replace(value).is_some() {
                    return None;
                }
                attrs = &attrs[attribute[0].len()..];
                if !attrs.is_empty() && !attrs.starts_with(char::is_whitespace) {
                    return None;
                }
                attrs = attrs.trim_start();
            }
            let path = path.filter(|p| !p.is_empty() && p.len() <= 4096 && !p.chars().any(char::is_control))?;
            Some(FileCitation {
                range: citation.get(0)?.range(),
                path,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use droppen_sessions::mentions::{annotate_mentioned_files, candidate_paths, MAX_FILES_PER_ITEM};

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
        let _guard = droppen_sessions::index::exclusive().await;
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
        CodexSessions.record_owned(&paths, id).await.expect("record");
        let mine = find_codex_session(&paths, id).await.expect("the row");
        assert_eq!(mine.scope, SessionScope::Owned);
        assert_eq!(mine.server, "alice");
    }

    /// A rollout head is up to 512 KiB, so this is the read most worth not
    /// repeating after a restart — and the one whose payload has to survive a
    /// serialise/deserialise round trip that nothing else in this file does.
    #[tokio::test]
    async fn a_rollout_read_once_is_not_read_again_after_a_restart() {
        let _guard = droppen_sessions::index::exclusive().await;
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        droppen_sessions::index::install(&paths);

        let id = "durable-index-fixture";
        let (_dir, path) = rollout(&[
            r#"{"type":"session_meta","payload":{"cwd":"/w","id":"x"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"from the rollout"}]}}"#,
        ]);
        let first = file_meta(&path, id).await.expect("meta");
        assert_eq!(first.title_fallback.as_deref(), Some("from the rollout"));

        META_CACHE.clear();
        let index = droppen_sessions::index::open(&paths);
        let metadata = std::fs::metadata(&path).expect("stat");
        let planted = FileMeta {
            title_fallback: Some("read out of the index".to_string()),
            ..first.clone()
        };
        index.remember_summary(Source::Codex, id, droppen_sessions::index::Watermark::of(&path, &metadata), &planted);

        let after = file_meta(&path, id).await.expect("meta");
        assert_eq!(
            after.title_fallback.as_deref(),
            Some("read out of the index"),
            "the head is not parsed again for a rollout that has not moved"
        );
        assert_eq!(after.cwd.as_deref(), Some("/w"), "and the rest of the row round-trips intact");
    }

    /// What Codex says to the user: the speech the attention clock, the unread
    /// count and the push read out of its rollouts ([`Transcripts::spoken_texts`]).
    /// The sessions core's own tests read a stand-in; these pin Codex's rules.
    mod speech {
        use super::*;
        use droppen_sessions::attention::derive::{echo, spoken, AssistantEcho};
        use droppen_sessions::attention::unread::{refresh, Ledger};
        use droppen_sessions::util::try_parse;
        use droppen_sessions::Transcripts;

        const FALLBACK: &str = "2026-01-01T09:59:59.000Z";

        fn codex_echo(tail: &[String]) -> Option<AssistantEcho> {
            echo(&CodexSessions, tail)
        }

        fn lines(raw: &[&str]) -> Vec<String> {
            raw.iter().map(|l| (*l).to_string()).collect()
        }

        fn at(n: u32) -> String {
            format!("2026-01-01T10:{:02}:{:02}.000Z", n / 60, n % 60)
        }

        fn write(dir: &Path, lines: &[String]) -> std::path::PathBuf {
            let path = dir.join("t.jsonl");
            let mut body = lines.join("\n");
            body.push('\n');
            std::fs::write(&path, body).expect("write");
            path
        }

        fn size(path: &Path) -> u64 {
            std::fs::metadata(path).expect("stat").len()
        }

        async fn ledger(reader: &dyn Transcripts, path: &Path, read_at: Option<&str>) -> Ledger {
            refresh(reader, path, None, size(path), read_at, FALLBACK).await.expect("a ledger")
        }

        #[test]
        fn a_transcript_with_no_assistant_text_has_no_clock() {
            let tail = lines(&[r#"{"type":"user","timestamp":"2026-01-01T00:00:00.000Z","message":{"content":[{"type":"text","text":"go"}]}}"#]);
            assert_eq!(codex_echo(&tail), None);
        }

        /// A codex wrapper is instructions, for the echo as for the count.
        #[test]
        fn a_codex_wrapper_is_not_speech_for_either_reader() {
            let tail = lines(&[
                r#"{"type":"response_item","timestamp":"2026-01-01T00:00:01.000Z","payload":{"type":"message","role":"assistant","content":[{"text":"real"}]}}"#,
                r#"{"type":"response_item","timestamp":"2026-01-01T00:00:02.000Z","payload":{"type":"message","role":"assistant","content":[{"text":"<environment_context>x</environment_context>"}]}}"#,
            ]);
            assert_eq!(codex_echo(&tail).expect("an echo").text, "real");
            assert_eq!(spoken(&CodexSessions, &try_parse(&tail[1]).expect("parses")), None);
        }

        #[test]
        fn codex_reads_assistant_messages_and_ignores_the_rest() {
            let tail = lines(&[
                r#"{"type":"response_item","timestamp":"2026-01-01T00:00:01.000Z","payload":{"type":"message","role":"assistant","content":[{"text":"the answer"}]}}"#,
                r#"{"type":"response_item","timestamp":"2026-01-01T00:00:02.000Z","payload":{"type":"reasoning","summary":[{"text":"thinking"}]}}"#,
                r#"{"type":"response_item","timestamp":"2026-01-01T00:00:03.000Z","payload":{"type":"function_call","name":"exec_command","arguments":"{}"}}"#,
                r#"{"type":"event_msg","timestamp":"2026-01-01T00:00:04.000Z","payload":{"type":"token_count"}}"#,
            ]);
            let echo = codex_echo(&tail).expect("an echo");
            assert_eq!(echo.at.as_deref(), Some("2026-01-01T00:00:01.000Z"));
            assert_eq!(echo.text, "the answer");
        }

        #[test]
        fn a_codex_user_message_is_not_the_assistant() {
            let tail = lines(&[
                r#"{"type":"response_item","timestamp":"2026-01-01T00:00:01.000Z","payload":{"type":"message","role":"assistant","content":[{"text":"mine"}]}}"#,
                r#"{"type":"response_item","timestamp":"2026-01-01T00:00:02.000Z","payload":{"type":"message","role":"user","content":[{"text":"yours"}]}}"#,
            ]);
            assert_eq!(codex_echo(&tail).expect("an echo").text, "mine");
        }


        #[tokio::test]
        async fn codex_counts_assistant_text_parts_only() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = write(
                dir.path(),
                &[
                    format!(
                        r#"{{"type":"response_item","timestamp":"{}","payload":{{"type":"message","role":"user","content":[{{"text":"go"}}]}}}}"#,
                        at(0)
                    ),
                    format!(
                        r#"{{"type":"response_item","timestamp":"{}","payload":{{"type":"message","role":"assistant","content":[{{"type":"output_text","text":"the answer"}}]}}}}"#,
                        at(1)
                    ),
                    format!(
                        r#"{{"type":"response_item","timestamp":"{}","payload":{{"type":"reasoning","summary":[{{"text":"thinking"}}]}}}}"#,
                        at(2)
                    ),
                    format!(
                        r#"{{"type":"response_item","timestamp":"{}","payload":{{"type":"function_call","name":"exec_command","arguments":"{{}}"}}}}"#,
                        at(3)
                    ),
                    format!(
                        r#"{{"type":"event_msg","timestamp":"{}","payload":{{"type":"agent_message","message":"the answer"}}}}"#,
                        at(4)
                    ),
                    format!(
                        r#"{{"type":"response_item","timestamp":"{}","payload":{{"type":"message","role":"assistant","content":[{{"text":"<environment_context>x</environment_context>"}},{{"text":"more"}}]}}}}"#,
                        at(5)
                    ),
                ],
            );
            let ledger = ledger(&CodexSessions, &path, None).await;
            assert_eq!(ledger.stamps, vec![at(1), at(5)], "the event_msg echo and the wrapper are not messages");
            assert_eq!(ledger.count(Some(&at(1))), Some(1));
        }

    }

    /// Codex's token usage, one rollout entry at a time
    /// ([`Transcripts::usage_fold`]). The sessions core adds the folds up by day
    /// and model; these pin what each entry is.
    mod usage {
        use super::*;
        use droppen_sessions::Transcripts;
        use serde_json::json;

        /// What each entry of a literal rollout folds to, in order.
        fn folds(entries: Value) -> Vec<Option<TokenUsage>> {
            let mut fold = CodexSessions.usage_fold();
            entries
                .as_array()
                .expect("array")
                .iter()
                .map(|entry| fold.fold(entry.as_object().expect("object")))
                .collect()
        }

        fn tokens(date: &str, model: &str, [input, output, cache_read, cache_write]: [f64; 4]) -> Option<TokenUsage> {
            Some(TokenUsage {
                date: date.to_string(),
                model: model.to_string(),
                input,
                output,
                cache_read,
                cache_write,
            })
        }

        #[test]
        fn codex_usage_counts_per_turn_deltas_and_attributes_the_active_model() {
            let folded = folds(json!([
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
            assert_eq!(
                folded,
                vec![
                    None,
                    // fresh input = input - cached: 9865-7552, then 12395-7552
                    tokens("2026-07-22", "gpt-5.6-sol", [2313.0, 93.0, 7552.0, 0.0]),
                    tokens("2026-07-22", "gpt-5.6-sol", [4843.0, 38.0, 7552.0, 0.0]),
                ]
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
            let folded = folds(json!([repeated, repeated]));
            assert_eq!(folded, vec![tokens("2026-07-22", "codex", [100.0, 0.0, 0.0, 0.0]), None], "the default model until a turn_context is seen");
        }
    }

    fn session(lines: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let mut file = std::fs::File::create(&path).expect("create");
        for line in lines {
            writeln!(file, "{line}").expect("write");
        }
        (dir, path)
    }

    /// The cumulative total is spend, not occupancy: this entry reports 24M
    /// against a 258k window, and reading it would peg the ring forever.
    #[tokio::test]
    async fn codex_reads_the_turn_not_the_running_total() {
        let (_dir, path) = session(&[
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":258400,"last_token_usage":{"input_tokens":202004,"cached_input_tokens":11008,"output_tokens":629},"total_token_usage":{"input_tokens":24141990,"total_tokens":24251793}}}}"#,
        ]);
        let usage = read_codex_context(&path).await.expect("a reading");
        assert_eq!(usage.tokens, 202_004);
        assert_eq!(usage.window, Some(258_400), "codex states its own window, so nothing is inferred");
    }

    #[tokio::test]
    async fn codex_without_a_stated_window_leaves_it_to_the_model_id() {
        let (_dir, path) = session(&[
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1200,"output_tokens":10}}}}"#,
        ]);
        let usage = read_codex_context(&path).await.expect("a reading");
        assert_eq!(usage.tokens, 1_200);
        assert_eq!(usage.window, None);
    }

    /// The reading is looked for as far back as the context budget
    /// (`droppen_sessions::context::TAIL_BYTES`), further than this reader's own
    /// tail for discovery.
    #[tokio::test]
    async fn the_context_reading_is_found_further_back_than_the_discovery_tail() {
        let reading = r#"{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1200,"output_tokens":10}}}}"#;
        let filler = format!(r#"{{"type":"event_msg","payload":{{"type":"agent_message","message":"{}"}}}}"#, "x".repeat(1000));
        let lines: Vec<&str> = std::iter::once(reading).chain(std::iter::repeat_n(filler.as_str(), 90)).collect();
        let (_dir, path) = session(&lines);
        let behind = std::fs::metadata(&path).expect("size").len() - reading.len() as u64;
        assert!(TAIL_BYTES < behind && behind < droppen_sessions::context::TAIL_BYTES, "{behind}");
        assert_eq!(read_codex_context(&path).await.map(|usage| usage.tokens), Some(1_200));
    }

    #[test]
    fn codex_file_citations_preserve_the_entire_quoted_path() {
        let path = "/Users/ada/Downloads/Garden/每周花园记录/outputs/种植计划汇总版_20260907-0913/种植计划（汇总版）.xlsx";
        let text = format!("生成汇总版：:codex-file-citation{{path=\"{path}\" purpose=\"output\"}}");
        assert_eq!(candidate_paths(&text, file_citations), vec![path]);
        let path = "/home/me/weekly reports/strategy (verified) {final}.xlsx";
        let text = format!(":codex-file-citation{{purpose=\"output\" path=\"{path}\"}}");
        assert_eq!(
            candidate_paths(&text, file_citations),
            vec![path],
            "quoted punctuation is not trimmed and fragments are not rediscovered"
        );
    }

    #[test]
    fn codex_file_citations_decode_escapes_and_deduplicate_normal_mentions() {
        let text = r#":codex-file-citation{path="/home/me/report \"final\".txt" purpose="output"} :codex-file-citation{path="/home/me/a.md"} `/home/me/a.md`"#;
        assert_eq!(candidate_paths(text, file_citations), vec!["/home/me/report \"final\".txt", "/home/me/a.md"]);
        assert_eq!(
            candidate_paths(r#":codex-file-citation{ path = "~/\u62a5\u544a.xlsx" }"#, file_citations),
            vec!["~/报告.xlsx"]
        );
    }

    #[test]
    fn malformed_or_ambiguous_file_citations_are_not_interpreted() {
        for text in [
            r#":codex-file-citation{path="a.md" path="b.md"}"#,
            r#":codex-file-citation{path="a.md"garbage}"#,
            r#":codex-file-citation{purpose="path=a.md"}"#,
            r#":codex-file-citation{path="a.md""#,
            r#":codex-file-citation{path="bad\q.txt"}"#,
            r#":codex-file-citation{path="bad\u0000.txt"}"#,
            r#":codex-file-citation{path=""}"#,
        ] {
            assert!(file_citations(text).is_empty(), "{text}");
        }
    }

    #[tokio::test]
    async fn citations_use_the_same_existence_boundary_budget_and_deduplication() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let real = home.path().join("综合策略 (核查版).xlsx");
        std::fs::write(&real, b"PK\0test").unwrap();
        let reference = format!(
            ":codex-file-citation{{path={} purpose=\"output\"}}",
            serde_json::to_string(&real.to_string_lossy()).unwrap()
        );
        let missing = ":codex-file-citation{path=\"missing.xlsx\" purpose=\"output\"}";
        let forbidden = format!(":codex-file-citation{{path=\"{}\"}}", outside.path().display());
        let mut items = vec![
            make_item(
                Role::Assistant,
                ItemKind::Text,
                &format!("生成：{reference} {reference} {missing} {forbidden}"),
                None,
            ),
            make_item(Role::User, ItemKind::Text, &reference, None),
            make_item(Role::Assistant, ItemKind::Thinking, &reference, None),
        ];
        annotate_mentioned_files(home.path(), &mut items, home.path().to_str(), file_citations).await;
        let files = items[0].files.as_ref().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, real.to_string_lossy());
        assert_eq!(files[0].name, "综合策略 (核查版).xlsx");
        assert!(
            !items[0].text.contains(&reference),
            "a verified citation becomes a readable label, not raw internal markup"
        );
        assert!(items[0].text.contains(missing), "a missing reference is not silently hidden");
        assert!(items[0].text.contains(&forbidden), "an inaccessible reference is not silently hidden");
        assert_eq!(items[1].text, reference);
        assert!(items[1].files.is_none());
        assert!(items[2].files.is_none());
        annotate_mentioned_files(home.path(), &mut items, home.path().to_str(), file_citations).await;
        assert_eq!(
            items[0].files.as_ref().unwrap().len(),
            1,
            "reannotation does not duplicate existing chips"
        );
    }

    #[tokio::test]
    async fn over_budget_citations_keep_their_visible_reference() {
        let home = tempfile::tempdir().unwrap();
        let mut references = Vec::new();
        for n in 0..MAX_FILES_PER_ITEM + 1 {
            std::fs::write(home.path().join(format!("报告 {n}.txt")), "hi").unwrap();
            references.push(format!(":codex-file-citation{{path=\"报告 {n}.txt\"}}"));
        }
        let mut items = vec![make_item(Role::Assistant, ItemKind::Text, &references.join("\n"), None)];
        annotate_mentioned_files(home.path(), &mut items, home.path().to_str(), file_citations).await;
        assert_eq!(items[0].files.as_ref().unwrap().len(), MAX_FILES_PER_ITEM);
        assert!(items[0].text.contains(references.last().unwrap()));
    }

    /// Codex's parse hands its own directive to the crate: a file a message
    /// cites becomes a chip, and the directive its label.
    #[tokio::test]
    async fn a_file_a_codex_message_cites_becomes_a_chip() {
        let text = r#"wrote :codex-file-citation{path="~/report.txt" purpose="output"}"#;
        let line = serde_json::json!({"type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]}});
        let (dir, path) = rollout(&[&line.to_string()]);
        std::fs::write(dir.path().join("report.txt"), "hi").expect("write");
        let items = parse_codex_transcript(&Paths::new(dir.path().to_path_buf()), &path).await.expect("parse");
        let files: Vec<&str> = items[0].files.iter().flatten().map(|file| file.name.as_str()).collect();
        assert_eq!(files, ["report.txt"]);
        assert_eq!(items[0].text, r"wrote report\.txt");
    }
}
