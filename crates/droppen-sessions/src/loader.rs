//! One parse of a session's transcript, with the fingerprint of every prefix
//! of it, and the small cache of the last parses that every reader shares.
//!
//! It reads files and holds no live state, so it sits with the transcript
//! parsers: the live feed, the history routes and the cloud agent's streams
//! all read through it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use sha2::{Digest, Sha256};

use super::util::TranscriptItem;
use super::{Source, TranscriptSources};
use droppen_instance::paths::Paths;

/// How many of a client's newest items a resume re-sends: `K` in
/// `feed`'s module docs. Those are the items the parser may still
/// rewrite in place, so they are neither trusted nor fingerprinted.
pub const RESUME_OVERLAP: usize = 3;

/// One link of a transcript's fingerprint chain: 128 bits of SHA-256.
pub type Fp = [u8; 16];

fn truncate_digest(digest: &[u8]) -> Fp {
    let mut fp = [0u8; 16];
    fp.copy_from_slice(&digest[..16]);
    fp
}

/// The chain's first link, which is what makes a fingerprint name a *file* and
/// not just a run of items: a transcript copied elsewhere (an adoption) is
/// another file, and a client resuming against it gets a fresh window.
fn fp_seed(file_path: &Path) -> Fp {
    let mut hasher = Sha256::new();
    hasher.update(b"remote-hand transcript fingerprint v1\0");
    hasher.update(file_path.as_os_str().as_encoded_bytes());
    truncate_digest(&hasher.finalize())
}

fn fp_link(previous: &Fp, item: &TranscriptItem) -> Fp {
    let mut hasher = Sha256::new();
    hasher.update(previous);
    hasher.update(serde_json::to_vec(item).unwrap_or_default());
    truncate_digest(&hasher.finalize())
}

/// How a fingerprint is spelled on the wire and in a query string.
pub fn encode_fp(fp: &Fp) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(fp)
}

/// The fingerprint chain of `items` read from `file_path`: entry `i` covers
/// items `[0, i)`, so there are `items.len() + 1` of them.
pub fn fingerprint_chain(file_path: &Path, items: &[TranscriptItem]) -> Vec<Fp> {
    let mut chain = Vec::with_capacity(items.len() + 1);
    let mut link = fp_seed(file_path);
    chain.push(link);
    for item in items {
        link = fp_link(&link, item);
        chain.push(link);
    }
    chain
}

/// One parse of a transcript, plus the fingerprint of every prefix of it.
///
/// The chain is computed on first use rather than with the parse: the REST
/// detail and the subagent list read the same cache and never need it.
#[derive(Debug)]
pub struct Transcript {
    /// The normalized items.
    pub items: Arc<Vec<TranscriptItem>>,
    file_path: PathBuf,
    chain: std::sync::OnceLock<Arc<[Fp]>>,
}

impl Transcript {
    /// A parse of `file_path`.
    pub fn new(file_path: &Path, items: Arc<Vec<TranscriptItem>>) -> Self {
        Self {
            items,
            file_path: file_path.to_path_buf(),
            chain: std::sync::OnceLock::new(),
        }
    }

    /// How many items it holds.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The fingerprint chain; see [`fingerprint_chain`].
    pub fn chain(&self) -> &Arc<[Fp]> {
        self.chain.get_or_init(|| fingerprint_chain(&self.file_path, &self.items).into())
    }

    /// The wire fingerprint of items `[0, at)`, or `None` past the end.
    pub fn fp_at(&self, at: usize) -> Option<String> {
        self.chain().get(at).map(encode_fp)
    }

    /// The resume marker of a window that ends where the transcript does: the
    /// fingerprint of `[0, len - K)`.
    pub fn prefix_fp(&self) -> String {
        self.fp_at(self.len().saturating_sub(RESUME_OVERLAP)).unwrap_or_default()
    }

    /// The parse of its first `end` items, of the same file, so its chain is
    /// this one's up to `end`. Panics when `end` is past the last item.
    pub fn prefix(&self, end: usize) -> Self {
        Self::new(&self.file_path, Arc::new(self.items[..end].to_vec()))
    }
}

/// How many transcripts are remembered; `TRANSCRIPT_CACHE_MAX` in
/// the former TypeScript server's `session-feed.ts`.
const TRANSCRIPT_CACHE_MAX: usize = 8;

/// One remembered parse.
struct CachedTranscript {
    path: PathBuf,
    /// `len:mtime` at the time of the parse — [`super::file_signature`]'s value, so a
    /// file that grew, shrank or was rewritten in place never matches.
    sig: String,
    /// The items, and the fingerprint chain once anybody asked for it — kept
    /// with the parse so every client resuming against it shares one.
    transcript: Arc<Transcript>,
}

/// The last parse of each of the handful of sessions in play.
///
/// Two jobs:
///
/// * Skip the work. Every attached client, the REST detail route and the
///   history route all parse the *same* file at the same signature, and a long
///   transcript is hundreds of megabytes read from byte zero. One parse per
///   change is what the TypeScript's `cachedTranscript` bought too; those who
///   ask while it is being made wait for it ([`IN_FLIGHT`]).
/// * Survive a bad read. A file that vanished under a rotation, a permission
///   change, a disk error must not be reported as *a transcript with no items*:
///   every attached client would repaint an empty session over a perfectly good
///   one. The last good parse is answered instead — at whatever signature it
///   was taken — and the refresh that follows corrects it.
///
/// Most recently used last, like the `Map` insertion order the TypeScript's own
/// LRU leans on.
static TRANSCRIPTS: std::sync::LazyLock<Mutex<Vec<CachedTranscript>>> = std::sync::LazyLock::new(|| Mutex::new(Vec::new()));

fn remember_transcript(file_path: &Path, sig: &str, transcript: Arc<Transcript>) {
    let mut cache = TRANSCRIPTS.lock().expect("transcript cache poisoned");
    cache.retain(|entry| entry.path != file_path);
    cache.push(CachedTranscript {
        path: file_path.to_path_buf(),
        sig: sig.to_string(),
        transcript,
    });
    if cache.len() > TRANSCRIPT_CACHE_MAX {
        cache.remove(0);
    }
}

/// Forget the remembered parse of `file_path`, the way a restart does, for a
/// test of what a reader gets with nothing cached.
#[cfg(any(test, feature = "test-support"))]
pub fn forget_transcript(file_path: &Path) {
    TRANSCRIPTS.lock().expect("cache").retain(|entry| entry.path != file_path);
}

/// The remembered parse for `file_path`, at `sig` when one is demanded.
fn cached_transcript(file_path: &Path, sig: Option<&str>) -> Option<Arc<Transcript>> {
    let cache = TRANSCRIPTS.lock().expect("transcript cache poisoned");
    cache
        .iter()
        .find(|entry| entry.path == file_path && sig.is_none_or(|sig| entry.sig == sig))
        .map(|entry| entry.transcript.clone())
}

/// What one parse in flight comes to, for everyone waiting on it: the
/// transcript, or `None` when the file could not be read.
type Flight = Arc<tokio::sync::OnceCell<Option<Arc<Transcript>>>>;

/// The parses in flight, by the file and the signature they were asked at.
///
/// One parse per version of a transcript, however many readers want it. A
/// change to a live session reaches the feed's watcher, every attached
/// client's refresh, the history and first-unread routes and the cloud agent's
/// stream at about the same moment, and each asks for the new signature before
/// any parse of it is remembered. Each used to read the file from byte zero:
/// on a 480 MiB transcript one parse took 0.9 s, and six at once about 3 s
/// each and 15 s of CPU between them. Now whoever asks while a parse is in
/// flight waits for it and shares what it gives — the transcript, or the
/// failure that sends each of them to the last good parse.
///
/// Keyed on the signature as well as the path, so a caller only ever gets a
/// parse made for the version it asked about: one still running for an older
/// signature is never handed to a caller of a newer one, nor the other way
/// round. Two versions can be parsed at once.
static IN_FLIGHT: std::sync::LazyLock<Mutex<HashMap<(PathBuf, String), Flight>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// A caller's place on the parse of one version in flight, which tidies the
/// entry away when nobody else holds one — so the map holds only what is in
/// flight, not every version ever read. The same shape as the attention
/// ledger's `FlightGuard`.
struct FlightTicket {
    key: (PathBuf, String),
    flight: Option<Flight>,
}

impl FlightTicket {
    /// Join the parse of `file_path` at `sig`, making the entry if nobody is on it.
    fn join(file_path: &Path, sig: &str) -> Self {
        let key = (file_path.to_path_buf(), sig.to_string());
        let flight = IN_FLIGHT
            .lock()
            .expect("transcript in-flight map poisoned")
            .entry(key.clone())
            .or_default()
            .clone();
        Self { key, flight: Some(flight) }
    }

    fn flight(&self) -> &Flight {
        self.flight.as_ref().expect("held until dropped")
    }
}

impl Drop for FlightTicket {
    fn drop(&mut self) {
        // Let go first, so the last of several callers leaving at once sees
        // only the map's own reference, whichever order they take the lock in.
        drop(self.flight.take());
        let mut flights = IN_FLIGHT.lock().expect("transcript in-flight map poisoned");
        if flights.get(&self.key).is_some_and(|flight| Arc::strong_count(flight) == 1) {
            flights.remove(&self.key);
        }
    }
}

impl TranscriptSources {
    /// Normalize a transcript for the feed and for the history route.
    ///
    /// `sig` is the caller's [`super::file_signature`] for `file_path`, which is what makes
    /// the cache above safe: an unchanged file is never re-read.
    ///
    /// Shared as an `Arc` because the same parse is fanned out to every attached
    /// client and is the only thing a failed read can fall back on; a `Vec` would be
    /// deep-cloned on both paths.
    pub async fn load_items(&self, paths: &Paths, source: Source, file_path: &Path, sig: &str) -> Arc<Vec<TranscriptItem>> {
        self.load(paths, source, file_path, sig).await.items.clone()
    }

    /// [`TranscriptSources::load_items`], with the fingerprint chain the feed and
    /// the history route resume against.
    ///
    /// Callers asking at the same `sig` while it is being parsed wait for that
    /// one parse rather than making their own.
    pub async fn load(&self, paths: &Paths, source: Source, file_path: &Path, sig: &str) -> Arc<Transcript> {
        if let Some(transcript) = cached_transcript(file_path, Some(sig)) {
            return transcript;
        }
        let ticket = FlightTicket::join(file_path, sig);
        // Whoever runs this is the one parse; everyone else on the ticket
        // waits for it. Should that caller go away mid-parse, the next one
        // waiting runs it instead.
        let parsed = ticket
            .flight()
            .get_or_init(|| async {
                // A parse that finished, and left the map, between the look
                // above and the join.
                if let Some(transcript) = cached_transcript(file_path, Some(sig)) {
                    return Some(transcript);
                }
                match self.parse(paths, source, file_path).await {
                    Ok(items) => {
                        let transcript = Arc::new(Transcript::new(file_path, Arc::new(items)));
                        remember_transcript(file_path, sig, transcript.clone());
                        Some(transcript)
                    }
                    Err(error) => {
                        tracing::warn!(path = %file_path.display(), %error, "cannot read transcript; keeping the last good items");
                        None
                    }
                }
            })
            .await
            .clone();
        drop(ticket);
        // The fallback is looked up by each caller once the parse is over,
        // so it is the last good parse as of now.
        parsed.unwrap_or_else(|| cached_transcript(file_path, None).unwrap_or_else(|| Arc::new(Transcript::new(file_path, Arc::default()))))
    }

    /// A transcript read now, past the cache and without filling it: for a
    /// reader that walks many sessions once (the cloud agent's first copy of a
    /// machine's history), which would otherwise push out the few parses the
    /// live feeds are sharing. `None` when it cannot be read.
    pub async fn read_uncached(&self, paths: &Paths, source: Source, file_path: &Path) -> Option<Transcript> {
        let parsed = self
            .parse(paths, source, file_path)
            .await
            .map(|items| Transcript::new(file_path, Arc::new(items)));
        match parsed {
            Ok(transcript) => Some(transcript),
            Err(error) => {
                tracing::warn!(path = %file_path.display(), %error, "cannot read transcript");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
    use std::time::Duration;

    use futures_util::future::BoxFuture;
    use serde_json::{Map, Value};

    use super::*;
    use crate::context::ContextUsage;
    use crate::sources::test_reader::TestReader;
    use crate::util::{make_item, ItemKind, Role};
    use crate::{CliChoice, SessionStatus, SessionSummary, Transcripts};

    /// How long a [`Slow`] parse takes, on the tests' paused clock: every
    /// caller a test starts at once arrives while it runs, and a timeout
    /// shorter than it always fires first.
    const PARSE_TIME: Duration = Duration::from_millis(50);

    /// The core's stand-in reader, whose parses are counted, take
    /// [`PARSE_TIME`], fail while it is told to, and each give one item
    /// naming which parse it was.
    struct Slow {
        reader: TestReader,
        parses: AtomicUsize,
        failing: AtomicBool,
    }

    impl Slow {
        /// The reader, and the sources it is registered in.
        fn sources() -> (Arc<Self>, TranscriptSources) {
            let slow = Arc::new(Self {
                reader: TestReader(Source::Claude),
                parses: AtomicUsize::new(0),
                failing: AtomicBool::new(false),
            });
            let mut sources = TranscriptSources::default();
            sources.register(slow.clone());
            (slow, sources)
        }

        fn parses(&self) -> usize {
            self.parses.load(SeqCst)
        }
    }

    impl Transcripts for Slow {
        fn source(&self) -> Source {
            self.reader.source()
        }

        fn list<'a>(&'a self, paths: &'a Paths, bindings: &'a BTreeMap<String, String>) -> BoxFuture<'a, Vec<SessionSummary>> {
            self.reader.list(paths, bindings)
        }

        fn find<'a>(&'a self, paths: &'a Paths, id: &'a str) -> BoxFuture<'a, Option<SessionSummary>> {
            self.reader.find(paths, id)
        }

        fn find_root<'a>(&'a self, paths: &'a Paths, id: &'a str) -> BoxFuture<'a, Option<droppen_instance::paths::ClaudeRoot>> {
            self.reader.find_root(paths, id)
        }

        fn has_own_sessions<'a>(&'a self, paths: &'a Paths) -> BoxFuture<'a, bool> {
            self.reader.has_own_sessions(paths)
        }

        fn parse<'a>(&'a self, _paths: &'a Paths, _file_path: &'a Path) -> BoxFuture<'a, io::Result<Vec<TranscriptItem>>> {
            Box::pin(async move {
                let parse = self.parses.fetch_add(1, SeqCst) + 1;
                tokio::time::sleep(PARSE_TIME).await;
                if self.failing.load(SeqCst) {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
                Ok(vec![make_item(Role::User, ItemKind::Text, &format!("parse {parse}"), None)])
            })
        }

        fn cli_choice<'a>(&'a self, file_path: &'a Path) -> BoxFuture<'a, Option<CliChoice>> {
            self.reader.cli_choice(file_path)
        }

        fn context<'a>(&'a self, file_path: &'a Path) -> BoxFuture<'a, Option<ContextUsage>> {
            self.reader.context(file_path)
        }

        fn spoken_texts(&self, entry: &Map<String, Value>) -> Vec<String> {
            self.reader.spoken_texts(entry)
        }

        fn external_reason(&self) -> String {
            self.reader.external_reason()
        }

        fn live_block(&self, status: SessionStatus) -> Option<String> {
            self.reader.live_block(status)
        }

        fn foreign_sessions(&self) -> bool {
            self.reader.foreign_sessions()
        }

        fn on_owned<'a>(&'a self, paths: &'a Paths, id: &'a str) -> BoxFuture<'a, ()> {
            self.reader.on_owned(paths, id)
        }

        fn watch_dirs<'a>(&'a self, paths: &'a Paths) -> BoxFuture<'a, Vec<PathBuf>> {
            self.reader.watch_dirs(paths)
        }
    }

    fn paths() -> Paths {
        Paths::new(PathBuf::from("/loader-test-home"))
    }

    /// The text of a transcript's one item: which parse it came from.
    fn made_by(transcript: &Transcript) -> &str {
        transcript.items.first().map_or("", |item| item.text.as_str())
    }

    /// Whether a parse of `file_path` is still on the in-flight map, at any
    /// signature.
    fn in_flight(file_path: &Path) -> bool {
        IN_FLIGHT.lock().expect("in flight").keys().any(|(path, _)| path == file_path)
    }

    /// However many callers ask for one version while it is parsed, it is
    /// parsed once and they all hold that one parse; once it is over, the map
    /// of parses in flight has let go of it.
    #[tokio::test(start_paused = true)]
    async fn callers_at_one_signature_share_one_parse() {
        let (slow, sources) = Slow::sources();
        let paths = paths();
        let file = Path::new("/loader-test/one-signature.jsonl");
        forget_transcript(file);

        let loads = (0..8).map(|_| sources.load(&paths, Source::Claude, file, "100:1"));
        let loaded = futures_util::future::join_all(loads).await;
        assert_eq!(slow.parses(), 1, "eight callers, one parse");
        assert!(
            loaded.iter().all(|transcript| Arc::ptr_eq(transcript, &loaded[0])),
            "the same parse for all"
        );
        assert_eq!(made_by(&loaded[0]), "parse 1");
        assert!(!in_flight(file));

        let again = sources.load(&paths, Source::Claude, file, "100:1").await;
        assert!(Arc::ptr_eq(&again, &loaded[0]), "and remembered, as before");
        assert_eq!(slow.parses(), 1);
    }

    /// Two versions asked for at once are two parses, and each caller holds
    /// the one made for the signature it asked at, never the other.
    #[tokio::test(start_paused = true)]
    async fn each_signature_has_its_own_parse() {
        let (slow, sources) = Slow::sources();
        let paths = paths();
        let file = Path::new("/loader-test/two-signatures.jsonl");
        forget_transcript(file);

        let sigs = ["100:1", "200:2", "100:1", "200:2", "200:2", "100:1"];
        let loads = sigs.map(|sig| sources.load(&paths, Source::Claude, file, sig));
        let loaded = futures_util::future::join_all(loads).await;
        assert_eq!(slow.parses(), 2, "one parse per signature");
        let (older, newer) = (&loaded[0], &loaded[1]);
        assert!(!Arc::ptr_eq(older, newer));
        for (sig, transcript) in sigs.iter().zip(&loaded) {
            let own = if *sig == "100:1" { older } else { newer };
            assert!(Arc::ptr_eq(transcript, own), "a caller at {sig} holds its own signature's parse");
        }
        assert_eq!(made_by(older), "parse 1", "the first asked for is the first parsed");
        assert!(!in_flight(file));
    }

    /// A parse that fails is shared too: it is tried once for everyone
    /// waiting, and each of them gets the last good parse, as a lone caller
    /// always has. The next caller at that signature tries again.
    #[tokio::test(start_paused = true)]
    async fn a_failed_parse_is_shared_and_falls_back_to_the_last_good_one() {
        let (slow, sources) = Slow::sources();
        let paths = paths();
        let file = Path::new("/loader-test/failed.jsonl");
        forget_transcript(file);
        let good = sources.load(&paths, Source::Claude, file, "100:1").await;

        slow.failing.store(true, SeqCst);
        let loads = (0..4).map(|_| sources.load(&paths, Source::Claude, file, "200:2"));
        let loaded = futures_util::future::join_all(loads).await;
        assert_eq!(slow.parses(), 2, "the good parse, and one failed one for all four");
        assert!(
            loaded.iter().all(|transcript| Arc::ptr_eq(transcript, &good)),
            "the last good parse for all"
        );
        assert!(!in_flight(file));

        sources.load(&paths, Source::Claude, file, "200:2").await;
        assert_eq!(slow.parses(), 3, "a failure is not remembered");

        forget_transcript(file);
        let nothing = sources.load(&paths, Source::Claude, file, "200:2").await;
        assert!(nothing.is_empty(), "with nothing good to fall back on, no items");
    }

    /// The caller running the parse can go away mid-parse (a client that
    /// disconnected, a request that timed out): one still waiting runs it
    /// instead, rather than waiting for nothing.
    #[tokio::test(start_paused = true)]
    async fn a_parse_whose_caller_went_away_is_run_by_one_still_waiting() {
        let (slow, sources) = Slow::sources();
        let paths = paths();
        let file = Path::new("/loader-test/abandoned.jsonl");
        forget_transcript(file);

        let (abandoned, waiter) = tokio::join!(
            tokio::time::timeout(PARSE_TIME / 5, sources.load(&paths, Source::Claude, file, "100:1")),
            sources.load(&paths, Source::Claude, file, "100:1"),
        );
        assert!(abandoned.is_err(), "the first caller gave up");
        assert_eq!(slow.parses(), 2, "the parse it left was run again");
        assert_eq!(made_by(&waiter), "parse 2");
        assert!(!in_flight(file));
    }
}
