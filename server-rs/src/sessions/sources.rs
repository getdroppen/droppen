//! The port sessions reads each CLI's transcripts through.
//!
//! Every session is a file some CLI wrote, in that CLI's format and in that
//! CLI's place, and the providers sit above sessions. So sessions declares
//! what it needs of a CLI's transcripts as [`Transcripts`], each provider
//! implements it next to its own parser, and the entry hands the readers in as
//! one [`TranscriptSources`]. The list, the detail, the feed and the cloud
//! streams then pick a reader by the session's [`Source`] and name no CLI.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde_json::{Map, Value};

use super::context::ContextUsage;
use super::sidecars::{Checklists, Subagents};
use super::util::TranscriptItem;
use super::{CliChoice, SessionStatus, SessionSummary, Source};
use droppen_base::time::mtime_ms;
use droppen_instance::paths::{ClaudeRoot, Paths};

/// One CLI's transcripts: where its sessions are, and how one is read.
///
/// Every method takes the instance's [`Paths`], because the roots a CLI's
/// files live in belong to the instance asking, and a test asks under its own.
pub trait Transcripts: Send + Sync {
    /// The source this reader answers for, which is its key in
    /// [`TranscriptSources`].
    fn source(&self) -> Source;

    /// Every visible session, in the CLI's own order. `bindings` is the
    /// session-to-account map, which only a caller above sessions can read;
    /// a reader whose sessions take accounts fills `account_id` from it.
    fn list<'a>(&'a self, paths: &'a Paths, bindings: &'a BTreeMap<String, String>) -> BoxFuture<'a, Vec<SessionSummary>>;

    /// One session's list row, or `None` when there is no such session.
    fn find<'a>(&'a self, paths: &'a Paths, id: &'a str) -> BoxFuture<'a, Option<SessionSummary>>;

    /// Where one session's transcript is, or `None` when there is no such
    /// session. The row's own `file_path` unless the reader can find the file
    /// without building the row.
    fn find_file<'a>(&'a self, paths: &'a Paths, id: &'a str) -> BoxFuture<'a, Option<PathBuf>> {
        Box::pin(async move { self.find(paths, id).await.map(|summary| PathBuf::from(summary.file_path)) })
    }

    /// The items the session view draws, read from the transcript at
    /// `file_path`.
    fn parse<'a>(&'a self, paths: &'a Paths, file_path: &'a Path) -> BoxFuture<'a, std::io::Result<Vec<TranscriptItem>>>;

    /// The model and effort the session last ran with, read from the
    /// transcript at `file_path`.
    fn cli_choice<'a>(&'a self, file_path: &'a Path) -> BoxFuture<'a, Option<CliChoice>>;

    /// How full the session's context window was on its last request, read
    /// from the transcript at `file_path`.
    fn context<'a>(&'a self, file_path: &'a Path) -> BoxFuture<'a, Option<ContextUsage>>;

    /// The texts in one transcript entry that are the agent *speaking to the
    /// user*, trimmed, in the order they were written.
    ///
    /// **The one definition** behind the badge, the unread count and the push
    /// notification ([`crate::sessions::attention::unread`] counts these;
    /// [`crate::sessions::attention::derive::echo`] takes the newest), so the
    /// three can never disagree about whether something was said. One entry
    /// per transcript row the session view draws for it. Thinking, tool calls
    /// and a subagent talking to itself are the session working, not speech.
    ///
    /// remote-hand's own rows (task notifications, hand-backs) are user-side
    /// entries and never reach this test.
    fn spoken_texts(&self, entry: &Map<String, Value>) -> Vec<String>;

    /// Whether a run the driver reaped after the transcript's last write
    /// settles a tail that still reads as running. True for a CLI whose
    /// process can die without writing its turn's end; see
    /// [`super::apply_driver`].
    fn settles_reaped_runs(&self) -> bool {
        false
    }

    /// Why a session of this CLI that this instance did not start cannot be
    /// sent to, in one line for the composer.
    fn external_reason(&self) -> String;

    /// Why a session this instance owns cannot be sent to while its
    /// transcript reads `status`, or `None` when that blocks nothing: the
    /// live-process arm of [`TranscriptSources::sendability`].
    fn live_block(&self, status: SessionStatus) -> Option<String> {
        let _ = status;
        None
    }

    /// The Claude root a session's detail reports, or `None` for a CLI whose
    /// sessions live in no Claude root.
    fn detail_root(&self, summary: &SessionSummary) -> Option<ClaudeRoot> {
        let _ = summary;
        None
    }

    /// Whether a session of this CLI that this instance did not start was run
    /// in the CLI itself, outside Droppen. False for a CLI only Droppen runs:
    /// its other sessions are another instance's.
    fn foreign_sessions(&self) -> bool {
        false
    }

    /// The directories a new transcript of this CLI can appear in, for the
    /// session list's watcher. A watch on a missing directory fails, so a
    /// reader whose directory may not exist yet makes it here.
    fn watch_dirs<'a>(&'a self, paths: &'a Paths) -> BoxFuture<'a, Vec<PathBuf>>;

    /// The subagents this CLI's sessions spawn, when it keeps their
    /// transcripts.
    fn subagents(&self) -> Option<&dyn Subagents> {
        None
    }

    /// The task checklists this CLI keeps for its sessions, when it keeps any.
    fn checklists(&self) -> Option<&dyn Checklists> {
        None
    }

    /// The transcripts that count toward the token-usage stats and were
    /// written at or after `cutoff_ms`. None by default: a CLI whose
    /// transcripts report no tokens this server can price adds nothing.
    fn usage_files<'a>(&'a self, paths: &'a Paths, cutoff_ms: f64) -> BoxFuture<'a, Vec<UsageFile>> {
        let _ = (paths, cutoff_ms);
        Box::pin(std::future::ready(Vec::new()))
    }

    /// A fold of one transcript's token usage, fresh for each file.
    fn usage_fold(&self) -> Box<dyn UsageFold> {
        Box::new(NoUsage)
    }
}

/// A transcript inside the usage window, with the mtime its fold is cached on.
#[derive(Debug, Clone)]
pub struct UsageFile {
    /// Where it is.
    pub path: PathBuf,
    /// Its mtime, in milliseconds.
    pub mtime: f64,
}

impl UsageFile {
    /// `path`, when it was written at or after `cutoff_ms`.
    pub async fn stat(path: PathBuf, cutoff_ms: f64) -> Option<Self> {
        let metadata = tokio::fs::metadata(&path).await.ok()?;
        let mtime = mtime_ms(&metadata);
        (mtime >= cutoff_ms).then_some(Self { path, mtime })
    }
}

/// The tokens one request used, as a transcript records them: what the usage
/// stats add up by day and model.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenUsage {
    /// The UTC calendar day of the entry, `YYYY-MM-DD`.
    pub date: String,
    /// The model that served the request.
    pub model: String,
    /// Fresh input tokens.
    pub input: f64,
    /// Generated output tokens.
    pub output: f64,
    /// Tokens served from the prompt cache.
    pub cache_read: f64,
    /// Tokens written into the prompt cache.
    pub cache_write: f64,
}

impl TokenUsage {
    /// The four counts added up.
    pub fn total(&self) -> f64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// One transcript read for its token usage an entry at a time, from its first
/// line on, so a fold may carry what an earlier entry said.
pub trait UsageFold: Send {
    /// The tokens `entry` adds, if any.
    fn fold(&mut self, entry: &Map<String, Value>) -> Option<TokenUsage>;
}

/// The fold of a transcript that reports no tokens this server can price.
struct NoUsage;

impl UsageFold for NoUsage {
    fn fold(&mut self, _entry: &Map<String, Value>) -> Option<TokenUsage> {
        None
    }
}

/// The reader of every source, keyed by [`Source`].
///
/// Kept in [`Source`]'s order, so a list of every source comes out in the same
/// order whichever reader was registered first: the list's sort keeps that
/// order between sessions updated at the same instant.
#[derive(Clone, Default)]
pub struct TranscriptSources(Arc<BTreeMap<Source, Arc<dyn Transcripts>>>);

impl TranscriptSources {
    /// Add `reader` under its own source, in place of any reader there.
    pub fn register(&mut self, reader: Arc<dyn Transcripts>) {
        Arc::make_mut(&mut self.0).insert(reader.source(), reader);
    }

    /// Every reader, in [`Source`]'s order.
    pub fn iter(&self) -> impl Iterator<Item = &dyn Transcripts> {
        self.0.values().map(|reader| reader.as_ref())
    }

    /// The reader of `source`, when one is registered.
    pub fn get(&self, source: Source) -> Option<&dyn Transcripts> {
        self.0.get(&source).map(|reader| reader.as_ref())
    }

    /// The sessions of `source`, or of every source when `None`, one source
    /// after the other. Every source is read at once.
    pub async fn list(&self, paths: &Paths, bindings: &BTreeMap<String, String>, source: Option<Source>) -> Vec<SessionSummary> {
        let readers: Vec<&Arc<dyn Transcripts>> = match source {
            Some(source) => self.0.get(&source).into_iter().collect(),
            None => self.0.values().collect(),
        };
        futures_util::future::join_all(readers.into_iter().map(|reader| reader.list(paths, bindings)))
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    /// Where the transcript of session `id` of `source` is; see
    /// [`Transcripts::find_file`].
    pub async fn find_file(&self, paths: &Paths, source: Source, id: &str) -> Option<PathBuf> {
        self.get(source)?.find_file(paths, id).await
    }

    /// See [`Transcripts::settles_reaped_runs`]; false for a source with no
    /// reader.
    pub fn settles_reaped_runs(&self, source: Source) -> bool {
        self.get(source).is_some_and(|reader| reader.settles_reaped_runs())
    }

    /// See [`Transcripts::external_reason`]. A source with no reader lists no
    /// sessions, so nothing is refused with the fallback wording.
    pub fn external_reason(&self, source: Source) -> String {
        match self.get(source) {
            Some(reader) => reader.external_reason(),
            None => format!("this {source} session was started elsewhere; continue it where it was started"),
        }
    }

    /// See [`Transcripts::detail_root`]; `None` for a source with no reader.
    pub fn detail_root(&self, source: Source, summary: &SessionSummary) -> Option<ClaudeRoot> {
        self.get(source)?.detail_root(summary)
    }

    /// See [`Transcripts::foreign_sessions`]; false for a source with no
    /// reader.
    pub fn foreign_sessions(&self, source: Source) -> bool {
        self.get(source).is_some_and(|reader| reader.foreign_sessions())
    }

    /// Every reader's [`Transcripts::watch_dirs`], in [`Source`]'s order.
    pub async fn watch_dirs(&self, paths: &Paths) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        for reader in self.iter() {
            dirs.extend(reader.watch_dirs(paths).await);
        }
        dirs
    }

    /// See [`Transcripts::subagents`]; `None` for a source with no reader.
    pub fn subagents(&self, source: Source) -> Option<&dyn Subagents> {
        self.get(source)?.subagents()
    }

    /// See [`Transcripts::checklists`]; `None` for a source with no reader.
    pub fn checklists(&self, source: Source) -> Option<&dyn Checklists> {
        self.get(source)?.checklists()
    }

    /// See [`Transcripts::cli_choice`].
    pub async fn cli_choice(&self, source: Source, file_path: &Path) -> Option<CliChoice> {
        self.get(source)?.cli_choice(file_path).await
    }

    /// See [`Transcripts::context`].
    pub async fn context(&self, source: Source, file_path: &Path) -> Option<ContextUsage> {
        self.get(source)?.context(file_path).await
    }

    /// `file_path` read by `source`'s reader, past every cache. A source with
    /// no reader cannot be read, which is an error like an unreadable file.
    pub async fn parse(&self, paths: &Paths, source: Source, file_path: &Path) -> std::io::Result<Vec<TranscriptItem>> {
        match self.get(source) {
            Some(reader) => reader.parse(paths, file_path).await,
            None => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!("no transcript reader for {source}"),
            )),
        }
    }
}
