//! What the link's streams copy, read from this server's sessions: the agent's
//! [`StreamSource`] over the session list, the transcript parser and its
//! cache. The account bindings, the drivers and the drafts sit above cloud,
//! so the entry hands them in ([`LiveSessions`]).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde_json::Value;

use crate::agent::streams::source::{DraftFeed, Fingerprints, Items, ListedSession, SessionRow, StreamSource, Transcript};
use crate::paths::Paths;
use crate::sessions::loader::{self, encode_fp, Fp, RESUME_OVERLAP};
use crate::sessions::util::TranscriptItem;
use crate::sessions::{DriverStates, ScopeFilter, SessionScope, SessionSummary, Source};

/// What the session list and the drafts need from above cloud.
pub trait LiveSessions: Send + Sync {
    /// Which account each session is bound to, by session key.
    fn bindings(&self) -> BoxFuture<'_, BTreeMap<String, String>>;
    /// What the drivers know about the sessions they run, read now.
    fn driver_states(&self) -> DriverStates;
    /// The draft of session `id` of `source` as it changes, when that session
    /// publishes one.
    fn drafts(&self, source: Source, id: &str) -> Option<Box<dyn DraftFeed>>;
}

/// The streams' [`StreamSource`]: this server's sessions, as its own session
/// list and feeds read them.
pub struct SessionStreams {
    paths: Arc<Paths>,
    live: Arc<dyn LiveSessions>,
}

impl SessionStreams {
    /// The sessions under `paths`, with the bindings, drivers and drafts of
    /// `live`.
    pub fn new(paths: Arc<Paths>, live: Arc<dyn LiveSessions>) -> Self {
        Self { paths, live }
    }
}

/// `source` as a [`Source`], the way a stream row's name is read back.
fn parse_source(source: &str) -> Option<Source> {
    serde_json::from_value(Value::String(source.to_string())).ok()
}

impl StreamSource for SessionStreams {
    fn sessions(&self, external: bool) -> BoxFuture<'_, Vec<Box<dyn ListedSession>>> {
        Box::pin(async move {
            let bindings = self.live.bindings().await;
            let drivers = self.live.driver_states();
            let filter = if external { ScopeFilter::All } else { ScopeFilter::Owned };
            crate::sessions::list_and_filter(&self.paths, &bindings, &drivers, None, None, filter)
                .await
                .into_iter()
                .map(|summary| {
                    Box::new(Listed {
                        summary,
                        paths: self.paths.clone(),
                    }) as Box<dyn ListedSession>
                })
                .collect()
        })
    }

    fn knows(&self, source: &str) -> bool {
        parse_source(source).is_some()
    }

    fn drafts(&self, source: &str, id: &str) -> Option<Box<dyn DraftFeed>> {
        self.live.drafts(parse_source(source)?, id)
    }
}

impl SessionRow for SessionSummary {
    fn source(&self) -> &str {
        self.source.as_str()
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn updated_at(&self) -> &str {
        &self.updated_at
    }
    fn owned(&self) -> bool {
        self.scope == SessionScope::Owned
    }
    fn json(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }
}

/// A row of the session list, with where its transcript is parsed from.
struct Listed {
    summary: SessionSummary,
    paths: Arc<Paths>,
}

impl SessionRow for Listed {
    fn source(&self) -> &str {
        self.summary.source()
    }
    fn id(&self) -> &str {
        self.summary.id()
    }
    fn updated_at(&self) -> &str {
        self.summary.updated_at()
    }
    fn owned(&self) -> bool {
        self.summary.owned()
    }
    fn json(&self) -> Option<String> {
        self.summary.json()
    }
}

impl ListedSession for Listed {
    fn signature(&self) -> BoxFuture<'_, Option<String>> {
        Box::pin(crate::sessions::file_signature(Path::new(&self.summary.file_path)))
    }

    fn transcript<'a>(&'a self, sig: &'a str) -> BoxFuture<'a, Arc<dyn Transcript>> {
        Box::pin(async move {
            let transcript: Arc<dyn Transcript> =
                loader::load_transcript(&self.paths, self.summary.source, Path::new(&self.summary.file_path), sig).await;
            transcript
        })
    }

    fn read_transcript(&self) -> BoxFuture<'_, Option<Arc<dyn Transcript>>> {
        Box::pin(async move {
            loader::read_transcript_uncached(&self.paths, self.summary.source, Path::new(&self.summary.file_path))
                .await
                .map(|transcript| Arc::new(transcript) as Arc<dyn Transcript>)
        })
    }
}

impl Transcript for loader::Transcript {
    fn items(&self) -> &dyn Items {
        &*self.items
    }
    fn chain(&self) -> &dyn Fingerprints {
        loader::Transcript::chain(self)
    }
    fn prefix(&self, end: usize) -> Arc<dyn Transcript> {
        Arc::new(loader::Transcript::prefix(self, end))
    }
}

impl Items for Vec<TranscriptItem> {
    fn len(&self) -> usize {
        Vec::len(self)
    }
    fn json(&self, i: usize) -> Value {
        serde_json::to_value(&self[i]).unwrap_or(Value::Null)
    }
    fn size(&self, i: usize) -> usize {
        serde_json::to_vec(&self[i]).map_or(0, |b| b.len())
    }
    fn timestamp(&self, i: usize) -> Option<&str> {
        self[i].timestamp.0.as_deref()
    }
}

/// A transcript's chain as [`loader::Transcript::chain`] keeps it, or as
/// [`loader::fingerprint_chain`] returns it.
impl<C: std::ops::Deref<Target = [Fp]> + Send + Sync> Fingerprints for C {
    fn len(&self) -> usize {
        <[Fp]>::len(self)
    }
    fn at(&self, at: usize) -> Option<String> {
        self.get(at).map(encode_fp)
    }
    fn resume(&self, end: usize) -> Option<String> {
        self.get(end.saturating_sub(RESUME_OVERLAP)).map(encode_fp)
    }
}
