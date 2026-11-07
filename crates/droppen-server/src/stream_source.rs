//! What the link's streams copy, read from this server's sessions: the agent's
//! [`StreamSource`] over the session list, the transcript parser and its
//! cache. The account bindings, the drivers and the drafts are the drive's,
//! which `app` hands in ([`LiveSessions`]). It is the entry's, so neither
//! cloud nor the agent names the sessions crate for it.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde_json::Value;

use droppen_agent::streams::source::{DraftFeed, Fingerprints, Items, ListedSession, SessionRow, StreamSource, Transcript};
use droppen_instance::paths::Paths;
use droppen_sessions::loader::{self, encode_fp, RESUME_OVERLAP};
use droppen_sessions::{DriverStates, ScopeFilter, SessionScope, SessionSummary, Source, TranscriptSources};

/// What the session list and the drafts need of the drive.
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
    transcripts: TranscriptSources,
    live: Arc<dyn LiveSessions>,
}

impl SessionStreams {
    /// The sessions under `paths`, read through `transcripts`, with the
    /// bindings, drivers and drafts of `live`.
    pub fn new(paths: Arc<Paths>, transcripts: TranscriptSources, live: Arc<dyn LiveSessions>) -> Self {
        Self { paths, transcripts, live }
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
            self.transcripts
                .list_and_filter(&self.paths, &bindings, &drivers, None, None, filter)
                .await
                .into_iter()
                .map(|summary| {
                    Box::new(Listed {
                        summary,
                        paths: self.paths.clone(),
                        transcripts: self.transcripts.clone(),
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

/// A row of the session list, with where its transcript is parsed from.
struct Listed {
    summary: SessionSummary,
    paths: Arc<Paths>,
    transcripts: TranscriptSources,
}

impl SessionRow for Listed {
    fn source(&self) -> &str {
        self.summary.source.as_str()
    }
    fn id(&self) -> &str {
        &self.summary.id
    }
    fn updated_at(&self) -> &str {
        &self.summary.updated_at
    }
    fn owned(&self) -> bool {
        self.summary.scope == SessionScope::Owned
    }
    fn json(&self) -> Option<String> {
        serde_json::to_string(&self.summary).ok()
    }
}

impl ListedSession for Listed {
    fn signature(&self) -> BoxFuture<'_, Option<String>> {
        Box::pin(droppen_sessions::file_signature(Path::new(&self.summary.file_path)))
    }

    fn transcript<'a>(&'a self, sig: &'a str) -> BoxFuture<'a, Arc<dyn Transcript>> {
        Box::pin(async move {
            let transcript = self
                .transcripts
                .load(&self.paths, self.summary.source, Path::new(&self.summary.file_path), sig)
                .await;
            Arc::new(LoadedTranscript(transcript)) as Arc<dyn Transcript>
        })
    }

    fn read_transcript(&self) -> BoxFuture<'_, Option<Arc<dyn Transcript>>> {
        Box::pin(async move {
            self.transcripts
                .read_uncached(&self.paths, self.summary.source, Path::new(&self.summary.file_path))
                .await
                .map(|transcript| Arc::new(LoadedTranscript(Arc::new(transcript))) as Arc<dyn Transcript>)
        })
    }
}

/// One parse of a session's transcript as the agent's streams read it: the
/// loader's parse, whose items and chain the agent's [`Transcript`],
/// [`Items`] and [`Fingerprints`] answer from. The agent cannot name the
/// loader, nor the loader the agent, so the entry holds the one in the other.
struct LoadedTranscript(Arc<loader::Transcript>);

impl Transcript for LoadedTranscript {
    fn items(&self) -> &dyn Items {
        self
    }
    fn chain(&self) -> &dyn Fingerprints {
        self
    }
    fn prefix(&self, end: usize) -> Arc<dyn Transcript> {
        Arc::new(LoadedTranscript(Arc::new(loader::Transcript::prefix(&self.0, end))))
    }
}

/// A transcript's items.
impl Items for LoadedTranscript {
    fn len(&self) -> usize {
        self.0.items.len()
    }
    fn json(&self, i: usize) -> Value {
        serde_json::to_value(&self.0.items[i]).unwrap_or(Value::Null)
    }
    fn size(&self, i: usize) -> usize {
        serde_json::to_vec(&self.0.items[i]).map_or(0, |b| b.len())
    }
    fn timestamp(&self, i: usize) -> Option<&str> {
        self.0.items[i].timestamp.0.as_deref()
    }
}

/// A transcript's chain, as [`loader::Transcript::chain`] keeps it.
impl Fingerprints for LoadedTranscript {
    fn len(&self) -> usize {
        self.0.chain().len()
    }
    fn at(&self, at: usize) -> Option<String> {
        self.0.chain().get(at).map(encode_fp)
    }
    fn resume(&self, end: usize) -> Option<String> {
        self.0.chain().get(end.saturating_sub(RESUME_OVERLAP)).map(encode_fp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use droppen_sessions::util::TranscriptItem;
    use droppen_agent::streams::test_transcripts::{TestItems, TestTranscript};

    /// The agent's streams tests read transcripts made the way the loader
    /// makes them (`droppen_agent::streams::test_transcripts`). This holds a
    /// loader's transcript, read through [`LoadedTranscript`] as the streams
    /// read it, against that stand-in for the same items: the same items,
    /// chain, fingerprints and resume markers, of the whole and of a prefix.
    /// A change to how the loader chains or spells a fingerprint fails here
    /// until the stand-in follows it.
    #[test]
    fn a_loaded_transcript_reads_as_the_streams_stand_in_does() {
        let path = Path::new("/tmp/t.jsonl");
        let items: Vec<TranscriptItem> = (0..6)
            .map(|i| {
                serde_json::from_value(serde_json::json!({
                    "role": if i % 2 == 0 { "user" } else { "assistant" }, "kind": "text", "text": format!("{i}:{}", "x".repeat(i * 7)),
                    "detail": null, "toolName": null, "meta": null, "isError": false, "question": null,
                    "timestamp": if i == 2 { Value::Null } else { Value::String(format!("2026-10-1{i}T08:00:00.000Z")) },
                    "truncated": false,
                }))
                .unwrap()
            })
            .collect();
        let loaded = LoadedTranscript(Arc::new(loader::Transcript::new(path, Arc::new(items.clone()))));
        let stand_in = TestTranscript::new(path, TestItems(items.iter().map(|item| serde_json::to_value(item).unwrap()).collect()));
        let same = |real: &dyn Transcript, test: &dyn Transcript| {
            assert_eq!(real.len(), test.len());
            for i in 0..real.len() {
                assert_eq!(real.items().json(i), test.items().json(i), "item {i}");
                assert_eq!(real.items().size(i), test.items().size(i), "item {i}");
                assert_eq!(real.items().timestamp(i), test.items().timestamp(i), "item {i}");
            }
            assert_eq!(real.chain().len(), real.len() + 1);
            assert_eq!(real.chain().len(), test.chain().len());
            for at in 0..real.chain().len() + 2 {
                assert_eq!(real.chain().at(at), test.chain().at(at), "entry {at}");
                assert_eq!(real.chain().resume(at), test.chain().resume(at), "resume at {at}");
            }
        };
        same(&loaded, &stand_in);
        same(&*loaded.prefix(3), &*stand_in.prefix(3));
    }
}
