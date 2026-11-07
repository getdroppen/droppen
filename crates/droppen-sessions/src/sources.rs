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

    /// The Claude root session `id`'s transcript is in, found as
    /// [`Self::find_file`] finds it, or `None` when there is no such session.
    /// A CLI whose sessions live in no Claude root answers `None`.
    fn find_root<'a>(&'a self, paths: &'a Paths, id: &'a str) -> BoxFuture<'a, Option<ClaudeRoot>>;

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
    /// notification ([`crate::attention::unread`] counts these;
    /// [`crate::attention::derive::echo`] takes the newest), so the
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
    /// live-process arm of [`TranscriptSources::sendability`]. Every reader
    /// answers, so a new one cannot let a send through by leaving it out.
    fn live_block(&self, status: SessionStatus) -> Option<String>;

    /// The Claude root a session's detail reports, or `None` for a CLI whose
    /// sessions live in no Claude root.
    fn detail_root(&self, summary: &SessionSummary) -> Option<ClaudeRoot> {
        let _ = summary;
        None
    }

    /// Whether a session of this CLI that this instance did not start was run
    /// in the CLI itself, outside Droppen. A CLI only Droppen runs lists every
    /// session as owned, so it answers false and is never asked about an
    /// external one.
    fn foreign_sessions(&self) -> bool;

    /// What this CLI adds once this instance has recorded making session
    /// `id`, run by [`super::owned_sessions::OwnedSessions::record`] right
    /// after the row. Codex writes the thread's claim in the shared data
    /// directory, without which the sibling instances go on listing the
    /// thread; a CLI the row alone settles adds nothing. Every reader answers,
    /// so a new one cannot record a row without saying what it needs.
    fn on_owned<'a>(&'a self, paths: &'a Paths, id: &'a str) -> BoxFuture<'a, ()>;

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

    /// Fill in `children` on `sessions`, every one of them this CLI's: the
    /// subagents each delegated work to, as a tree under it, read against the
    /// reader's read marks (see [`super::children`]). A CLI that keeps no
    /// links between its sessions leaves each with none.
    fn children<'a>(&'a self, _paths: &'a Paths, sessions: Vec<&'a mut SessionSummary>, _marks: &'a BTreeMap<String, String>) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            for session in sessions {
                session.children = Vec::new();
            }
        })
    }

    /// Whether this server's own root of this CLI holds a session transcript,
    /// which says the install has been used even where no ownership record
    /// does. No default: each reader says whether its CLI's sessions count.
    fn has_own_sessions<'a>(&'a self, paths: &'a Paths) -> BoxFuture<'a, bool>;

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
    /// Add `reader` under its own source.
    ///
    /// # Panics
    ///
    /// When that source has a reader already. A source has exactly one, so no
    /// reader can stand in for another: the step after an ownership row
    /// ([`Transcripts::on_owned`]) is always its own CLI's.
    pub fn register(&mut self, reader: Arc<dyn Transcripts>) {
        let source = reader.source();
        assert!(!self.0.contains_key(&source), "a second transcript reader for {source}");
        Arc::make_mut(&mut self.0).insert(source, reader);
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

    /// The Claude root of session `id` of `source`; see
    /// [`Transcripts::find_root`].
    pub async fn find_root(&self, paths: &Paths, source: Source, id: &str) -> Option<ClaudeRoot> {
        self.get(source)?.find_root(paths, id).await
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

/// A reader for the sessions core's own tests.
///
/// The core reads every CLI through [`Transcripts`], and the real readers are
/// the providers', which the core's tests do not name. This one stands in for
/// a CLI. Its lines are shaped like Claude Code's, so a fixture still reads as
/// a transcript, and it reads as little of them as those tests need: an
/// `assistant` entry speaks the trimmed, non-empty text of its text blocks,
/// and an entry's `usage` object is the tokens it used, field for field. What
/// a real CLI counts as speech or as usage is pinned next to its reader.
#[cfg(test)]
pub(crate) mod test_reader {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use futures_util::future::BoxFuture;
    use serde_json::{Map, Value};

    use super::{TokenUsage, TranscriptSources, Transcripts, UsageFold};
    use crate::context::ContextUsage;
    use crate::util::{num, TranscriptItem};
    use crate::{CliChoice, SessionStatus, SessionSummary, Source};
    use droppen_instance::paths::Paths;

    /// The stand-in, answering for `source`.
    pub(crate) struct TestReader(pub(crate) Source);

    impl TestReader {
        /// The readers a core test reads its rows through: this one, for the
        /// claude rows the fixtures make.
        pub(crate) fn sources() -> TranscriptSources {
            let mut sources = TranscriptSources::default();
            sources.register(Arc::new(TestReader(Source::Claude)));
            sources
        }
    }

    impl Transcripts for TestReader {
        fn source(&self) -> Source {
            self.0
        }

        fn list<'a>(&'a self, _paths: &'a Paths, _bindings: &'a BTreeMap<String, String>) -> BoxFuture<'a, Vec<SessionSummary>> {
            Box::pin(std::future::ready(Vec::new()))
        }

        fn find<'a>(&'a self, _paths: &'a Paths, _id: &'a str) -> BoxFuture<'a, Option<SessionSummary>> {
            Box::pin(std::future::ready(None))
        }

        // A stand-in finds no session, so none has a root.
        fn find_root<'a>(&'a self, _paths: &'a Paths, _id: &'a str) -> BoxFuture<'a, Option<droppen_instance::paths::ClaudeRoot>> {
            Box::pin(std::future::ready(None))
        }

        /// False: a stand-in has no root of the server's own to look in.
        fn has_own_sessions<'a>(&'a self, _paths: &'a Paths) -> BoxFuture<'a, bool> {
            Box::pin(std::future::ready(false))
        }

        fn parse<'a>(&'a self, _paths: &'a Paths, _file_path: &'a Path) -> BoxFuture<'a, std::io::Result<Vec<TranscriptItem>>> {
            Box::pin(std::future::ready(Ok(Vec::new())))
        }

        fn cli_choice<'a>(&'a self, _file_path: &'a Path) -> BoxFuture<'a, Option<CliChoice>> {
            Box::pin(std::future::ready(None))
        }

        fn context<'a>(&'a self, _file_path: &'a Path) -> BoxFuture<'a, Option<ContextUsage>> {
            Box::pin(std::future::ready(None))
        }

        fn spoken_texts(&self, entry: &Map<String, Value>) -> Vec<String> {
            if entry.get("type").and_then(Value::as_str) != Some("assistant") {
                return Vec::new();
            }
            let blocks = entry.get("message").and_then(|message| message.get("content")).and_then(Value::as_array);
            blocks
                .into_iter()
                .flatten()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
                .collect()
        }

        fn external_reason(&self) -> String {
            "this session was started elsewhere".to_string()
        }

        fn live_block(&self, _status: SessionStatus) -> Option<String> {
            None
        }

        fn foreign_sessions(&self) -> bool {
            false
        }

        fn on_owned<'a>(&'a self, _paths: &'a Paths, _id: &'a str) -> BoxFuture<'a, ()> {
            Box::pin(std::future::ready(()))
        }

        fn watch_dirs<'a>(&'a self, _paths: &'a Paths) -> BoxFuture<'a, Vec<PathBuf>> {
            Box::pin(std::future::ready(Vec::new()))
        }

        fn usage_fold(&self) -> Box<dyn UsageFold> {
            Box::new(PlainUsage)
        }
    }

    /// An entry's `usage` object: `date`, `model` and the four counts by
    /// their [`TokenUsage`] names.
    struct PlainUsage;

    impl UsageFold for PlainUsage {
        fn fold(&mut self, entry: &Map<String, Value>) -> Option<TokenUsage> {
            let usage = entry.get("usage")?.as_object()?;
            Some(TokenUsage {
                date: usage.get("date")?.as_str()?.to_owned(),
                model: usage.get("model")?.as_str()?.to_owned(),
                input: num(usage.get("input")),
                output: num(usage.get("output")),
                cache_read: num(usage.get("cache_read")),
                cache_write: num(usage.get("cache_write")),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_reader::TestReader;
    use super::*;

    /// One reader per source: a second would stand in for the first, and so
    /// would its step after an ownership row.
    #[test]
    #[should_panic(expected = "a second transcript reader for claude")]
    fn a_source_takes_one_reader() {
        let mut sources = TranscriptSources::default();
        sources.register(Arc::new(TestReader(Source::Claude)));
        sources.register(Arc::new(TestReader(Source::Claude)));
    }
}
