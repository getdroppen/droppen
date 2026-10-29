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

use super::util::TranscriptItem;
use super::{SessionSummary, Source};
use crate::paths::Paths;

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
