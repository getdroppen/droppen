//! One parse of a session's transcript, with the fingerprint of every prefix
//! of it, and the small cache of the last parses that every reader shares.
//!
//! It reads files and holds no live state, so it sits with the transcript
//! parsers: the live feed, the history routes and the cloud agent's streams
//! all read through it.
//!
//! # Reading on
//!
//! A live transcript is appended to every few seconds, and each version used
//! to be parsed from byte zero: half a second and more for one of 480 MiB, by
//! the feed and the cloud agent on every change. A parse whose reader can go
//! on from it ([`crate::resume::Resumable`]) keeps what the next one needs
//! next to it in the cache ([`Checkpoint`], for a text of at least
//! [`RESUME_MIN_BYTES`]), and the next version is read from there: only the
//! lines appended, with the fingerprint chain made on from the last one's
//! ([`Transcript::resumed`]). That is the same parse a read of the whole
//! file makes, which the readers' tests prove, `DROPPEN_PARSE_ORACLE=1`
//! checks as it runs, and a read of the whole file every
//! [`RESUMES_BEFORE_WHOLE`] parses, or every [`WHOLE_READ_EVERY`], backs up.
//! Whatever cannot be shown to be the last version grown (by the signatures,
//! [`crate::growth`], and the bytes,
//! [`crate::resume::ResumableLines::reopen`]) is read whole, which is no
//! failure; a file that cannot be read fails as it always did, and keeps the
//! checkpoint for the next try, as does a parse whose caller went away.
//!
//! The parses of one file that take a checkpoint go one at a time
//! ([`LANES`]): one that comes while another goes on waits for it, and goes
//! on from what it read.

use std::any::Any;
use std::borrow::Borrow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine;
use sha2::{Digest, Sha256};

use super::resume::{ReadPoint, ResumableLines, Resumed, READ_ON_MAX};
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

/// The link after `previous` for `item` as its transcript wrote it: without
/// the mentioned-file chips, which say what the filesystem held at parse time
/// ([`TranscriptItem::as_transcribed`]). Two parses of an unchanged file chain
/// the same, whatever was created or deleted in between.
fn fp_link(previous: &Fp, item: &TranscriptItem) -> Fp {
    let mut hasher = Sha256::new();
    hasher.update(previous);
    hasher.update(serde_json::to_vec(item.as_transcribed()).unwrap_or_default());
    truncate_digest(&hasher.finalize())
}

/// How a fingerprint is spelled on the wire and in a query string.
pub fn encode_fp(fp: &Fp) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(fp)
}

/// The fingerprint chain of `items` read from `file_path`: entry `i` covers
/// items `[0, i)`, so there are `items.len() + 1` of them. `items` are a
/// [`Transcript`]'s, each behind its `Arc`, or a reader's own.
pub fn fingerprint_chain(file_path: &Path, items: &[impl Borrow<TranscriptItem>]) -> Vec<Fp> {
    let mut chain = Vec::with_capacity(items.len() + 1);
    let mut link = fp_seed(file_path);
    chain.push(link);
    for item in items {
        link = fp_link(&link, item.borrow());
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
    /// The normalized items, each behind an `Arc` of its own, so that a
    /// transcript made of another's items ([`Transcript::prefix`]) shares
    /// them rather than copying them.
    pub items: Arc<Vec<Arc<TranscriptItem>>>,
    file_path: PathBuf,
    chain: std::sync::OnceLock<Arc<[Fp]>>,
}

impl Transcript {
    /// A parse of `file_path`, of items already shared, in a vector with no
    /// room to spare.
    ///
    /// Private, so that holds: a vector made with `collect` from a reader's
    /// own can keep that reader's buffer ([`Transcript::parsed`]).
    fn new(file_path: &Path, items: Arc<Vec<Arc<TranscriptItem>>>) -> Self {
        Self {
            items,
            file_path: file_path.to_path_buf(),
            chain: std::sync::OnceLock::new(),
        }
    }

    /// A parse of `file_path` that went on from `base`, and whose items below
    /// `changed_from` are `base`'s as written ([`crate::resume::Resumed`]).
    /// When `base`'s chain is made, so is this one's, from it: its links up to
    /// there are copied, and only the items after are hashed. Otherwise it is
    /// made on first use, as any parse's is.
    fn resumed(file_path: &Path, mut items: Vec<Arc<TranscriptItem>>, base: &Transcript, changed_from: usize) -> Self {
        items.shrink_to_fit();
        let transcript = Self::new(file_path, Arc::new(items));
        if let Some(base) = base.chain.get().filter(|chain| changed_from < chain.len()) {
            let mut chain = Vec::with_capacity(transcript.items.len() + 1);
            chain.extend_from_slice(&base[..=changed_from]);
            let mut link = base[changed_from];
            for item in &transcript.items[changed_from..] {
                link = fp_link(&link, item);
                chain.push(link);
            }
            let _ = transcript.chain.set(chain.into());
        }
        transcript
    }

    /// A parse of `file_path`, of the items its reader returned.
    pub fn parsed(file_path: &Path, items: Vec<TranscriptItem>) -> Self {
        // Not `collect`: it would keep the reader's buffer, room for every
        // item itself, to hold one pointer per item — tens of megabytes held
        // for nothing by a long transcript.
        let mut shared = Vec::with_capacity(items.len());
        shared.extend(items.into_iter().map(Arc::new));
        Self::new(file_path, Arc::new(shared))
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
    ///
    /// Made of this one's items, not copies of them, and of its chain once
    /// that is made: no item is cloned and none is hashed again.
    pub fn prefix(&self, end: usize) -> Self {
        let prefix = Self::new(&self.file_path, Arc::new(self.items[..end].to_vec()));
        if let Some(chain) = self.chain.get() {
            let _ = prefix.chain.set(chain[..=end].into());
        }
        prefix
    }
}

/// How many transcripts are remembered; `TRANSCRIPT_CACHE_MAX` in
/// the former TypeScript server's `session-feed.ts`.
const TRANSCRIPT_CACHE_MAX: usize = 8;

/// How long a transcript's text has to be before its parse keeps what the
/// next one needs to go on from it ([`Checkpoint`]). A shorter one is read
/// whole in a few milliseconds, and what a parse keeps (every tool call's id,
/// among others) is not worth holding for it.
pub const RESUME_MIN_BYTES: u64 = 8 * 1024 * 1024;

/// How many parses in a row may go on from the one before before a parse
/// reads the file whole again, whatever it could go on from. A backstop for
/// what proving the file only grew cannot see ([`crate::resume`]): a rewrite
/// that keeps both ends of what was read, or a reader that went wrong.
pub const RESUMES_BEFORE_WHOLE: u32 = 64;

/// How long parses may go on from the one before before a parse reads the
/// file whole again: the same backstop as [`RESUMES_BEFORE_WHOLE`], for a
/// session written to slowly, which could otherwise go on from a rewrite the
/// proof cannot see for hours.
pub const WHOLE_READ_EVERY: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// What a parse keeps for the next one to go on from, next to it in the
/// cache: where it stopped, the reader's own state, how many parses in a row
/// went on from one before, and when the file was last read whole
/// ([`RESUMES_BEFORE_WHOLE`], [`WHOLE_READ_EVERY`]). The next parse takes it
/// rather than copying it.
struct Checkpoint {
    point: ReadPoint,
    state: Box<dyn Any + Send>,
    resumes: u32,
    whole_at: std::time::Instant,
}

/// One remembered parse.
struct CachedTranscript {
    path: PathBuf,
    /// `len:mtime` at the time of the parse — [`super::file_signature`]'s value, so a
    /// file that grew, shrank or was rewritten in place never matches.
    sig: String,
    /// When the parse started, from [`PARSES_STARTED`].
    started: u64,
    /// The items, and the fingerprint chain once anybody asked for it — kept
    /// with the parse so every client resuming against it shares one.
    transcript: Arc<Transcript>,
    /// What the next parse goes on from, while nobody has taken it.
    checkpoint: Option<Checkpoint>,
}

/// Every version's place in the order its parse started: taken when the first
/// caller at a signature starts the parse ([`Flight::started`]).
///
/// Parses of two versions of a file can overlap, and the older one can finish
/// last: it took longer, or its caller went away and a waiter started it again.
/// A parse is never remembered over one that started after it, so a parse of
/// what the file was never pushes out one of what it became: the newer parse
/// stays what the fallback answers and what a caller at the new signature
/// finds. The order parses started, not the signature's mtime: a file put back
/// with an older mtime is still a newer read of it.
static PARSES_STARTED: AtomicU64 = AtomicU64::new(0);

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
///   was taken — and a reader that remembers versions is told it is not the
///   one it asked for ([`TranscriptSources::load_checked`]), so it asks again.
///
/// Most recently used last, like the `Map` insertion order the TypeScript's own
/// LRU leans on.
static TRANSCRIPTS: std::sync::LazyLock<Mutex<Vec<CachedTranscript>>> = std::sync::LazyLock::new(|| Mutex::new(Vec::new()));

/// Remember a parse of `file_path` at `sig` that started at `started`, unless
/// the one remembered for it started later ([`PARSES_STARTED`]); with it, what
/// the next parse goes on from. A parse left out drops its checkpoint, and so
/// does one pushed out of the cache.
fn remember_transcript(file_path: &Path, sig: &str, started: u64, transcript: Arc<Transcript>, checkpoint: Option<Checkpoint>) {
    let mut cache = TRANSCRIPTS.lock().expect("transcript cache poisoned");
    if cache.iter().any(|entry| entry.path == file_path && entry.started > started) {
        return;
    }
    cache.retain(|entry| entry.path != file_path);
    cache.push(CachedTranscript {
        path: file_path.to_path_buf(),
        sig: sig.to_string(),
        started,
        transcript,
        checkpoint,
    });
    if cache.len() > TRANSCRIPT_CACHE_MAX {
        cache.remove(0);
    }
}

/// The parse a parse of `file_path` at `sig` can go on from: the newest one
/// remembered, its checkpoint taken off it; otherwise how the file is read
/// whole.
///
/// Only when the file can be that parse's grown, as far as two signatures
/// can tell ([`crate::growth`]). One that cannot, and cannot be its older
/// self either, was rewritten (shorter, or with an older mtime, whatever its
/// two ends say): its checkpoint is dropped. A caller whose signature is the
/// older of the two leaves it where it is, for the next. A checkpoint past
/// either backstop is dropped too, and the file read whole.
fn take_base(file_path: &Path, sig: &str) -> Result<Taken, Mode> {
    if DISTRUSTED.lock().expect("distrusted poisoned").remove(file_path) {
        drop_checkpoint(file_path);
        return Err(Mode::Full);
    }
    let mut cache = TRANSCRIPTS.lock().expect("transcript cache poisoned");
    let Some(entry) = cache.iter_mut().find(|entry| entry.path == file_path && entry.checkpoint.is_some()) else {
        return Err(Mode::Full);
    };
    if crate::growth(&entry.sig, sig).is_none() {
        if crate::growth(sig, &entry.sig).is_none() {
            entry.checkpoint = None;
        }
        return Err(Mode::Fallback);
    }
    let checkpoint = entry.checkpoint.take().expect("looked for one");
    if checkpoint.resumes >= RESUMES_BEFORE_WHOLE || checkpoint.whole_at.elapsed() >= whole_read_every(file_path) {
        return Err(Mode::Full);
    }
    Ok(Taken {
        file_path: file_path.to_path_buf(),
        base: entry.transcript.clone(),
        checkpoint: Some(checkpoint),
    })
}

/// A checkpoint taken off the parse it was kept with, `base`, until the
/// parse going on from it hands the reader its state. Dropped before then
/// (the file could not be read, or the caller went away mid-read), it goes
/// back on `base`, for the next parse to go on from.
struct Taken {
    file_path: PathBuf,
    base: Arc<Transcript>,
    checkpoint: Option<Checkpoint>,
}

impl Drop for Taken {
    fn drop(&mut self) {
        if let Some(checkpoint) = self.checkpoint.take() {
            put_back(&self.file_path, &self.base, checkpoint);
        }
    }
}

/// Put `checkpoint` back on the parse it was taken off, `base`, while that is
/// still the one remembered and has none: a parse that went on from it could
/// not read the file, or went away, and the next one can still go on from it.
/// On any other parse it would be read on from the wrong place.
fn put_back(file_path: &Path, base: &Arc<Transcript>, checkpoint: Checkpoint) {
    let mut cache = TRANSCRIPTS.lock().expect("transcript cache poisoned");
    if let Some(entry) = cache
        .iter_mut()
        .find(|entry| entry.path == file_path && Arc::ptr_eq(&entry.transcript, base) && entry.checkpoint.is_none())
    {
        entry.checkpoint = Some(checkpoint);
    }
}

/// Drop the checkpoint of the parse of `file_path` remembered now, so the
/// next parse reads the file whole.
fn drop_checkpoint(file_path: &Path) {
    let mut cache = TRANSCRIPTS.lock().expect("transcript cache poisoned");
    if let Some(entry) = cache.iter_mut().find(|entry| entry.path == file_path) {
        entry.checkpoint = None;
    }
}

/// One lock per transcript that a parse which can go on from another holds
/// from before it takes a checkpoint until it has remembered what it read
/// ([`Lane`]).
///
/// Without it, a parse of one version that comes while another goes on from
/// the checkpoint finds none, and reads the file whole: with two readers on a
/// live session, one in fifteen versions; with a dozen, one in four. With it,
/// that parse waits for the one going on, and goes on from what it read.
/// Only parses of the same file wait on each other, and only those whose
/// reader can go on ([`crate::resume::Resumable`]).
static LANES: std::sync::LazyLock<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// A parse's turn in its transcript's lane ([`LANES`]), which tidies the
/// lane away when nobody else is in it or waiting.
struct Lane {
    path: PathBuf,
    turn: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl Lane {
    /// Wait for `file_path`'s lane.
    async fn enter(file_path: &Path) -> Self {
        let lane = LANES.lock().expect("lanes poisoned").entry(file_path.to_path_buf()).or_default().clone();
        Self {
            path: file_path.to_path_buf(),
            turn: Some(lane.lock_owned().await),
        }
    }
}

impl Drop for Lane {
    fn drop(&mut self) {
        drop(self.turn.take());
        let mut lanes = LANES.lock().expect("lanes poisoned");
        if lanes.get(&self.path).is_some_and(|lane| Arc::strong_count(lane) == 1) {
            lanes.remove(&self.path);
        }
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

/// The parse of one version in flight.
struct Flight {
    /// Its place in [`PARSES_STARTED`], taken when the flight is made. A
    /// waiter that runs the parse again, its first caller gone, keeps it.
    started: u64,
    /// What the parse comes to, for everyone waiting on it: the transcript, or
    /// `None` when the file could not be read.
    parsed: tokio::sync::OnceCell<Option<Arc<Transcript>>>,
}

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
static IN_FLIGHT: std::sync::LazyLock<Mutex<HashMap<(PathBuf, String), Arc<Flight>>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// A caller's place on the parse of one version in flight, which tidies the
/// entry away when nobody else holds one — so the map holds only what is in
/// flight, not every version ever read. The same shape as the attention
/// ledger's `FlightGuard`.
struct FlightTicket {
    key: (PathBuf, String),
    flight: Option<Arc<Flight>>,
}

impl FlightTicket {
    /// Join the parse of `file_path` at `sig`, making the entry if nobody is on it.
    fn join(file_path: &Path, sig: &str) -> Self {
        let key = (file_path.to_path_buf(), sig.to_string());
        let flight = IN_FLIGHT
            .lock()
            .expect("transcript in-flight map poisoned")
            .entry(key.clone())
            .or_insert_with(|| {
                Arc::new(Flight {
                    started: PARSES_STARTED.fetch_add(1, Ordering::Relaxed),
                    parsed: tokio::sync::OnceCell::new(),
                })
            })
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

/// The signature each transcript last failed to read at, until a read of it
/// works.
///
/// A reader that remembers versions tries one that failed again, later and
/// later (the feed's watcher), and a file that stays unreadable would
/// otherwise say so at every try. It is warned about once per version, and
/// logged at debug level after that.
static FAILED_READS: std::sync::LazyLock<Mutex<HashMap<PathBuf, String>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Note that `file_path` could not be read at `sig`: whether it is the first
/// time for that version since the last read of the file that worked.
fn first_failure(file_path: &Path, sig: &str) -> bool {
    let mut failed = FAILED_READS.lock().expect("failed reads poisoned");
    failed.insert(file_path.to_path_buf(), sig.to_string()).as_deref() != Some(sig)
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
    pub async fn load_items(&self, paths: &Paths, source: Source, file_path: &Path, sig: &str) -> Arc<Vec<Arc<TranscriptItem>>> {
        self.load(paths, source, file_path, sig).await.items.clone()
    }

    /// [`TranscriptSources::load_items`], with the fingerprint chain the feed and
    /// the history route resume against.
    ///
    /// Callers asking at the same `sig` while it is being parsed wait for that
    /// one parse rather than making their own.
    pub async fn load(&self, paths: &Paths, source: Source, file_path: &Path, sig: &str) -> Arc<Transcript> {
        self.load_checked(paths, source, file_path, sig).await.0
    }

    /// [`TranscriptSources::load`], and whether what it answers is the parse
    /// at `sig`: `false` when the read failed and the answer is the last good
    /// parse, of another version, or no items.
    ///
    /// For a reader that remembers which version it was last handed. What a
    /// failed read answers is not the version at `sig`, and taken for it,
    /// `sig` would not be read again until the file changed once more: for a
    /// session gone quiet, never.
    pub async fn load_checked(&self, paths: &Paths, source: Source, file_path: &Path, sig: &str) -> (Arc<Transcript>, bool) {
        if let Some(transcript) = cached_transcript(file_path, Some(sig)) {
            return (transcript, true);
        }
        let ticket = FlightTicket::join(file_path, sig);
        // Whoever runs this is the one parse; everyone else on the ticket
        // waits for it. Should that caller go away mid-parse, the next one
        // waiting runs it instead.
        let flight = ticket.flight();
        let parsed = flight
            .parsed
            .get_or_init(|| async {
                // A parse that finished, and left the map, between the look
                // above and the join.
                if let Some(transcript) = cached_transcript(file_path, Some(sig)) {
                    return Some(transcript);
                }
                // A parse that can go on from another waits its turn in the
                // file's lane, and keeps it until what it read is remembered.
                let resumable = self.get(source).is_some_and(|reader| reader.resumable().is_some());
                let _lane = match resumable {
                    true => Some(Lane::enter(file_path).await),
                    false => None,
                };
                if let Some(transcript) = cached_transcript(file_path, Some(sig)) {
                    return Some(transcript);
                }
                let began = std::time::Instant::now();
                match self.read_version(paths, source, file_path, sig).await {
                    Ok(read) => {
                        tracing::debug!(
                            path = %file_path.display(),
                            mode = read.mode.name(),
                            bytes = read.bytes,
                            low_water = read.low_water,
                            ms = began.elapsed().as_secs_f64() * 1e3,
                            "read a transcript"
                        );
                        #[cfg(any(test, feature = "test-support"))]
                        test_support::count(file_path, |reads| match read.mode {
                            Mode::Full => reads.full += 1,
                            Mode::Resume => reads.resumed += 1,
                            Mode::Fallback => reads.fallback += 1,
                        });
                        let transcript = Arc::new(read.transcript);
                        remember_transcript(file_path, sig, flight.started, transcript.clone(), read.checkpoint);
                        FAILED_READS.lock().expect("failed reads poisoned").remove(file_path);
                        if let Some(check) = read.check {
                            self.check_against_whole(paths, source, file_path, sig, &transcript, check);
                        }
                        Some(transcript)
                    }
                    Err(error) if first_failure(file_path, sig) => {
                        tracing::warn!(path = %file_path.display(), %error, "cannot read transcript; keeping the last good items");
                        None
                    }
                    Err(error) => {
                        tracing::debug!(path = %file_path.display(), %error, "still cannot read transcript; keeping the last good items");
                        None
                    }
                }
            })
            .await
            .clone();
        drop(ticket);
        // The fallback is looked up by each caller once the parse is over,
        // so it is the last good parse as of now.
        match parsed {
            Some(transcript) => (transcript, true),
            None => {
                let fallback = cached_transcript(file_path, None).unwrap_or_else(|| Arc::new(Transcript::new(file_path, Arc::default())));
                (fallback, false)
            }
        }
    }

    /// The parse of `file_path` at `sig`, when one is remembered. Never a
    /// read: for a reader waiting to try a version again that it could not
    /// read, which another reader may have read since.
    pub fn cached(&self, file_path: &Path, sig: &str) -> Option<Arc<Transcript>> {
        cached_transcript(file_path, Some(sig))
    }

    /// A transcript read now, past the cache and without filling it: for a
    /// reader that walks many sessions once (the cloud agent's first copy of a
    /// machine's history), which would otherwise push out the few parses the
    /// live feeds are sharing. `None` when it cannot be read.
    pub async fn read_uncached(&self, paths: &Paths, source: Source, file_path: &Path) -> Option<Transcript> {
        let parsed = self
            .parse(paths, source, file_path)
            .await
            .map(|items| Transcript::parsed(file_path, items));
        match parsed {
            Ok(transcript) => Some(transcript),
            Err(error) => {
                tracing::warn!(path = %file_path.display(), %error, "cannot read transcript");
                None
            }
        }
    }

    /// The parse of `file_path` the cache holds — it keeps one per path, and
    /// never one that started before the one it replaces ([`PARSES_STARTED`])
    /// — whatever signature it was taken at, and that signature; `None` with
    /// none. Never a read.
    ///
    /// For a reader that proves for itself how much of an older parse still
    /// stands — the history route does, against its client's fingerprint — so
    /// that a transcript appended to every few seconds does not cost it a parse
    /// of the whole file on every request.
    pub fn newest_cached(&self, file_path: &Path) -> Option<(String, Arc<Transcript>)> {
        let cache = TRANSCRIPTS.lock().expect("transcript cache poisoned");
        cache
            .iter()
            .find(|entry| entry.path == file_path)
            .map(|entry| (entry.sig.clone(), entry.transcript.clone()))
    }

    /// Read the version of `file_path` at `sig`: on from the newest parse
    /// remembered when its reader can ([`crate::resume::Resumable`]) and the
    /// file is that parse's grown, otherwise whole.
    ///
    /// A file that cannot be read is an error, as a whole read's is, and the
    /// parse it would have gone on from keeps its checkpoint ([`Taken`]):
    /// what was appended is read in full before the reader's state is touched
    /// ([`ResumableLines::reopen`]), so the next read can still go on from
    /// it, and so can one whose caller went away meanwhile. Anything else that
    /// keeps it from going on (a file rewritten, more appended than
    /// [`READ_ON_MAX`], a reader whose other inputs moved) is a read of the
    /// whole file instead.
    async fn read_version(&self, paths: &Paths, source: Source, file_path: &Path, sig: &str) -> std::io::Result<Read> {
        let Some(resumable) = self.get(source).and_then(|reader| reader.resumable()) else {
            let items = self.parse(paths, source, file_path).await?;
            return Ok(Read {
                transcript: Transcript::parsed(file_path, items),
                checkpoint: None,
                mode: Mode::Full,
                bytes: sig.split_once(':').and_then(|(len, _)| len.parse().ok()).unwrap_or(0),
                low_water: 0,
                check: None,
            });
        };
        let mode = match take_base(file_path, sig) {
            Err(mode) => mode,
            Ok(mut taken) => {
                // A check against the whole file has to know the reader's
                // other inputs held still: they are looked at before it reads
                // them.
                let inputs = match oracle_checks(file_path) {
                    true => Some(resumable.inputs(paths, file_path).await),
                    false => None,
                };
                let point = taken.checkpoint.as_ref().expect("taken").point.clone();
                #[cfg(any(test, feature = "test-support"))]
                test_support::pass(file_path, "reopen").await;
                // An error leaves the checkpoint with `taken`, which puts it back.
                match ResumableLines::reopen(file_path, &point, read_on_max(file_path)).await? {
                    None => {
                        taken.checkpoint = None;
                        Mode::Fallback
                    }
                    Some(lines) => {
                        let checkpoint = taken.checkpoint.take().expect("taken");
                        let (resumes, whole_at) = (checkpoint.resumes + 1, checkpoint.whole_at);
                        match resumable.resume(paths, file_path, lines, taken.base.items.to_vec(), checkpoint.state).await? {
                            Some(resumed) => {
                                let (end, low_water) = (resumed.point.end(), resumed.changed_from);
                                let mut read = Read::of(file_path, Mode::Resume, resumed, Some(&*taken.base), resumes, whole_at, point.end());
                                read.check = inputs.map(|inputs| Check {
                                    end,
                                    low_water,
                                    resumes,
                                    inputs,
                                });
                                return Ok(read);
                            }
                            None => Mode::Fallback,
                        }
                    }
                }
            }
        };
        let read = resumable.read(paths, file_path, ResumableLines::open(file_path).await?).await?;
        Ok(Read::of(file_path, mode, read, None, 0, std::time::Instant::now(), 0))
    }

    /// Check a parse that went on from the last one against one of the whole
    /// file up to where it stopped (`DROPPEN_PARSE_ORACLE`). They have to agree
    /// on every item as written, and so on the fingerprint chain, the one made
    /// on from the parse before included; the chips are left out, since a
    /// file a message names can come or go between the two. When they do not
    /// agree, it is logged and the checkpoint dropped: the next parse reads
    /// the file whole.
    ///
    /// A whole parse costs what going on saved, and a live session goes on
    /// every few seconds. So one check per transcript runs at a time, and a
    /// parse that goes on while one runs is not checked; and it runs on a
    /// blocking thread, not on one serving requests. Nor is a check whose
    /// reader's other inputs moved meanwhile ([`crate::resume::Resumable::inputs`]: a
    /// subagent's meta written, a chat's delivery recorded) taken to say
    /// anything: the two parses read different things.
    fn check_against_whole(&self, paths: &Paths, source: Source, file_path: &Path, sig: &str, resumed: &Arc<Transcript>, check: Check) {
        static CHECKING: std::sync::LazyLock<Mutex<std::collections::HashSet<PathBuf>>> = std::sync::LazyLock::new(Default::default);
        /// A transcript's place in [`CHECKING`], given up however the check ends.
        struct Checking(PathBuf);
        impl Drop for Checking {
            fn drop(&mut self) {
                CHECKING.lock().expect("checking poisoned").remove(&self.0);
            }
        }

        if !CHECKING.lock().expect("checking poisoned").insert(file_path.to_path_buf()) {
            tracing::debug!(path = %file_path.display(), "a check of a parse that went on is still running; this one goes unchecked");
            #[cfg(any(test, feature = "test-support"))]
            test_support::count(file_path, |reads| reads.skipped += 1);
            return;
        }
        let checking = Checking(file_path.to_path_buf());
        let (sources, paths, sig, resumed) = (self.clone(), paths.clone(), sig.to_string(), resumed.clone());
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let file_path = checking.0.clone();
            handle.block_on(async move {
                #[cfg(any(test, feature = "test-support"))]
                test_support::pass(&file_path, "check").await;
                let Some(resumable) = sources.get(source).and_then(|reader| reader.resumable()) else {
                    return;
                };
                let before = resumable.inputs(&paths, &file_path).await;
                let whole = match ResumableLines::open_upto(&file_path, check.end).await {
                    Ok(lines) => resumable.read(&paths, &file_path, lines).await,
                    Err(error) => Err(error),
                };
                let after = resumable.inputs(&paths, &file_path).await;
                // Nothing to compare with a file that cannot be read now.
                let Ok(whole) = whole else { return };
                if check.inputs != before || before != after {
                    tracing::debug!(path = %file_path.display(), "what the reader reads besides the transcript moved; the check says nothing");
                    #[cfg(any(test, feature = "test-support"))]
                    test_support::count(&file_path, |reads| reads.inconclusive += 1);
                    return;
                }
                let whole = Transcript::new(&file_path, Arc::new(whole.items));
                let (ours, theirs) = (resumed.chain(), whole.chain());
                let differs = (ours != theirs).then(|| ours.iter().zip(theirs.iter()).position(|(a, b)| a != b).unwrap_or(ours.len().min(theirs.len())));
                if let Some(link) = differs {
                    tracing::error!(
                        path = %file_path.display(),
                        %sig,
                        item = link.saturating_sub(1),
                        end = check.end,
                        low_water = check.low_water,
                        resumes = check.resumes,
                        items = resumed.len(),
                        whole = whole.len(),
                        "a parse that went on from the last one is not the whole file's; the next one reads it whole"
                    );
                    distrust(&file_path);
                }
                #[cfg(any(test, feature = "test-support"))]
                test_support::count(&file_path, |reads| {
                    reads.checked += 1;
                    reads.mismatched += u32::from(differs.is_some());
                });
            });
            drop(checking);
        });
    }
}

/// How a version of a transcript was read, for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Whole: there was no parse to go on from, or it had gone on as long as
    /// it may ([`RESUMES_BEFORE_WHOLE`], [`WHOLE_READ_EVERY`]).
    Full,
    /// On from the last parse.
    Resume,
    /// Whole, though there was a parse to go on from: the file was not that
    /// parse's grown, more was appended than [`READ_ON_MAX`], or the reader
    /// would not go on from it.
    Fallback,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Full => "full",
            Mode::Resume => "resume",
            Mode::Fallback => "fallback",
        }
    }
}

/// What a check of a parse that went on needs ([`TranscriptSources::check_against_whole`]).
struct Check {
    /// Where the parse stopped reading.
    end: u64,
    low_water: usize,
    resumes: u32,
    /// The reader's other inputs before the parse read them.
    inputs: u64,
}

/// A version of a transcript, read.
struct Read {
    transcript: Transcript,
    checkpoint: Option<Checkpoint>,
    mode: Mode,
    /// How many bytes of the text were read.
    bytes: u64,
    /// The lowest index whose item may differ from the parse this one went
    /// on from; 0 for a read of the whole file.
    low_water: usize,
    /// Its check against the whole file, when one is to be made.
    check: Option<Check>,
}

impl Read {
    /// `resumed`, read in `mode` from byte `from` on, going on from `base` if
    /// it did; its checkpoint kept when the text is long enough
    /// ([`RESUME_MIN_BYTES`]), counting `resumes` parses in a row that went
    /// on since the file was read whole at `whole_at`.
    fn of(file_path: &Path, mode: Mode, resumed: Resumed, base: Option<&Transcript>, resumes: u32, whole_at: std::time::Instant, from: u64) -> Self {
        let end = resumed.point.end();
        let low_water = resumed.changed_from;
        let transcript = match base {
            Some(base) => Transcript::resumed(file_path, resumed.items, base, low_water),
            None => {
                let mut items = resumed.items;
                items.shrink_to_fit();
                Transcript::new(file_path, Arc::new(items))
            }
        };
        let checkpoint = (end >= resume_min_bytes(file_path)).then(|| Checkpoint {
            point: resumed.point,
            state: resumed.state,
            resumes,
            whole_at,
        });
        Self {
            transcript,
            checkpoint,
            mode,
            bytes: end.saturating_sub(from),
            low_water,
            check: None,
        }
    }
}

/// The transcripts whose last parse that went on did not agree with a whole
/// one ([`TranscriptSources::check_against_whole`]): the next parse of each
/// reads the file whole, whatever checkpoint a parse that went on meanwhile
/// left.
static DISTRUSTED: std::sync::LazyLock<Mutex<std::collections::HashSet<PathBuf>>> = std::sync::LazyLock::new(Default::default);

/// Have the next parse of `file_path` read it whole.
fn distrust(file_path: &Path) {
    DISTRUSTED.lock().expect("distrusted poisoned").insert(file_path.to_path_buf());
    drop_checkpoint(file_path);
}

/// Whether a parse that went on from the last one is checked against a
/// parse of the whole file: with `DROPPEN_PARSE_ORACLE=1` in the server's
/// environment. Off by default, since a check costs the very parse going on
/// saves; a way to watch the readers on real transcripts.
static ORACLE: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var("DROPPEN_PARSE_ORACLE").is_ok_and(|value| value == "1"));

/// Whether parses of `file_path` that go on are checked ([`ORACLE`]).
fn oracle_checks(file_path: &Path) -> bool {
    #[cfg(any(test, feature = "test-support"))]
    if test_support::checked(file_path) {
        return true;
    }
    let _ = file_path;
    *ORACLE
}

/// How long `file_path`'s text has to be for its parse to keep a checkpoint.
fn resume_min_bytes(file_path: &Path) -> u64 {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(bytes) = test_support::knob(file_path, "threshold") {
        return bytes;
    }
    let _ = file_path;
    RESUME_MIN_BYTES
}

/// The most a parse of `file_path` reads on from the last one.
fn read_on_max(file_path: &Path) -> u64 {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(bytes) = test_support::knob(file_path, "read on") {
        return bytes;
    }
    let _ = file_path;
    READ_ON_MAX
}

/// How long parses of `file_path` may go on before one reads it whole.
fn whole_read_every(file_path: &Path) -> std::time::Duration {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(millis) = test_support::knob(file_path, "whole every") {
        return std::time::Duration::from_millis(millis);
    }
    let _ = file_path;
    WHOLE_READ_EVERY
}

/// Knobs, holds and counts for tests of what goes on from what, by path, so
/// tests running at once each keep to their own file.
#[cfg(any(test, feature = "test-support"))]
mod test_support {
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, LazyLock, Mutex};

    use super::{Gate, Reads};

    static KNOBS: LazyLock<Mutex<HashMap<(PathBuf, &'static str), u64>>> = LazyLock::new(Default::default);
    static CHECKED: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Default::default);
    static READS: LazyLock<Mutex<HashMap<PathBuf, Reads>>> = LazyLock::new(Default::default);
    static GATES: LazyLock<Mutex<HashMap<(PathBuf, &'static str), Arc<Gate>>>> = LazyLock::new(Default::default);

    pub(super) fn knob(file_path: &Path, name: &'static str) -> Option<u64> {
        KNOBS.lock().expect("knobs").get(&(file_path.to_path_buf(), name)).copied()
    }

    pub(super) fn set_knob(file_path: &Path, name: &'static str, value: u64) {
        KNOBS.lock().expect("knobs").insert((file_path.to_path_buf(), name), value);
    }

    pub(super) fn checked(file_path: &Path) -> bool {
        CHECKED.lock().expect("checked").contains(file_path)
    }

    pub(super) fn check(file_path: &Path) {
        CHECKED.lock().expect("checked").insert(file_path.to_path_buf());
    }

    pub(super) fn count(file_path: &Path, add: impl FnOnce(&mut Reads)) {
        add(READS.lock().expect("reads").entry(file_path.to_path_buf()).or_default());
    }

    pub(super) fn reads(file_path: &Path) -> Reads {
        READS.lock().expect("reads").get(file_path).copied().unwrap_or_default()
    }

    pub(super) fn gate(file_path: &Path, at: &'static str) -> Arc<Gate> {
        let gate = Arc::new(Gate {
            reached: tokio::sync::Semaphore::new(0),
            open: tokio::sync::Semaphore::new(0),
        });
        GATES.lock().expect("gates").insert((file_path.to_path_buf(), at), gate.clone());
        gate
    }

    /// Pass the gate held at `at` for `file_path`, if one is.
    pub(super) async fn pass(file_path: &Path, at: &'static str) {
        let gate = GATES.lock().expect("gates").get(&(file_path.to_path_buf(), at)).cloned();
        if let Some(gate) = gate {
            gate.reached.add_permits(1);
            if let Ok(permit) = gate.open.acquire().await {
                permit.forget();
            }
        }
    }
}

/// A point a test holds the parses of a transcript at ([`hold`]): each that
/// comes to it says so, and waits for a pass.
#[cfg(any(test, feature = "test-support"))]
pub struct Gate {
    reached: tokio::sync::Semaphore,
    open: tokio::sync::Semaphore,
}

#[cfg(any(test, feature = "test-support"))]
impl Gate {
    /// Wait for a parse to come to the gate; ten seconds, and the test fails
    /// rather than hangs.
    pub async fn reached(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(10), self.reached.acquire())
            .await
            .expect("a parse came to the gate within ten seconds")
            .expect("a gate is never closed")
            .forget();
    }

    /// Let one parse waiting at the gate, or the next to come, through.
    pub fn open(&self) {
        self.open.add_permits(1);
    }
}

/// Hold the parses of `file_path` at `at`: `"reopen"`, before one that goes
/// on reads what was appended; `"check"`, before a check of one against the
/// whole file.
#[cfg(any(test, feature = "test-support"))]
pub fn hold(file_path: &Path, at: &'static str) -> Arc<Gate> {
    test_support::gate(file_path, at)
}

/// How the versions of one transcript were read, for a test.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Reads {
    /// Whole, with nothing to go on from.
    pub full: u32,
    /// On from the last parse.
    pub resumed: u32,
    /// Whole, though there was a parse to go on from.
    pub fallback: u32,
    /// Parses that went on, checked against the whole file's.
    pub checked: u32,
    /// Of those, the ones that did not agree.
    pub mismatched: u32,
    /// Checks that said nothing: the reader's other inputs moved.
    pub inconclusive: u32,
    /// Parses that went on and went unchecked: one was being checked.
    pub skipped: u32,
}

/// How the versions of `file_path` were read so far.
#[cfg(any(test, feature = "test-support"))]
pub fn reads_of(file_path: &Path) -> Reads {
    test_support::reads(file_path)
}

/// Keep the checkpoints of `file_path`'s parses from `bytes` of text on,
/// rather than [`RESUME_MIN_BYTES`]: a test's transcript is short.
#[cfg(any(test, feature = "test-support"))]
pub fn resume_from(file_path: &Path, bytes: u64) {
    test_support::set_knob(file_path, "threshold", bytes);
}

/// Read on from the last parse of `file_path` at most `bytes`, rather than
/// [`READ_ON_MAX`].
#[cfg(any(test, feature = "test-support"))]
pub fn read_on_at_most(file_path: &Path, bytes: u64) {
    test_support::set_knob(file_path, "read on", bytes);
}

/// Read `file_path` whole once its last whole read is `every` old, rather
/// than [`WHOLE_READ_EVERY`].
#[cfg(any(test, feature = "test-support"))]
pub fn read_whole_every(file_path: &Path, every: std::time::Duration) {
    test_support::set_knob(file_path, "whole every", u64::try_from(every.as_millis()).unwrap_or(u64::MAX));
}

/// Check every parse of `file_path` that goes on against the whole file's,
/// as `DROPPEN_PARSE_ORACLE=1` checks every one.
#[cfg(any(test, feature = "test-support"))]
pub fn check_resumes(file_path: &Path) {
    test_support::check(file_path);
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
    use std::time::Duration;

    use futures_util::future::BoxFuture;

    use super::*;
    use crate::sources::test_reader::{go_stale, hook_parse, lie, move_inputs, LinesReader, TestReader};
    use crate::util::{make_item, ItemKind, Role};

    /// How long a hooked parse takes unless told otherwise, on the tests'
    /// paused clock: every caller a test starts at once arrives while it runs,
    /// and a timeout shorter than it always fires first.
    const PARSE_TIME: Duration = Duration::from_millis(50);

    /// The parses of one hooked file ([`hooked`]).
    struct Parses {
        count: AtomicUsize,
        failing: AtomicBool,
        /// How long each parse takes, the first first; [`PARSE_TIME`] past
        /// the end.
        times: Vec<Duration>,
    }

    impl Parses {
        fn count(&self) -> usize {
            self.count.load(SeqCst)
        }
    }

    /// Hook the stand-in reader's parse of `file`: each parse is counted,
    /// takes its turn's time in `times`, fails while `failing` is set, and
    /// otherwise gives one item naming which parse it was. Each test hooks a
    /// file of its own.
    fn hooked(file: &Path, times: &[Duration]) -> (Arc<Parses>, TranscriptSources) {
        forget_transcript(file);
        let parses = Arc::new(Parses {
            count: AtomicUsize::new(0),
            failing: AtomicBool::new(false),
            times: times.to_vec(),
        });
        let state = parses.clone();
        hook_parse(
            file,
            Arc::new(move || -> BoxFuture<'static, io::Result<Vec<TranscriptItem>>> {
                let state = state.clone();
                Box::pin(async move {
                    let parse = state.count.fetch_add(1, SeqCst) + 1;
                    tokio::time::sleep(state.times.get(parse - 1).copied().unwrap_or(PARSE_TIME)).await;
                    if state.failing.load(SeqCst) {
                        return Err(io::ErrorKind::PermissionDenied.into());
                    }
                    Ok(vec![make_item(Role::User, ItemKind::Text, &format!("parse {parse}"), None)])
                })
            }),
        );
        (parses, TestReader::sources())
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
        let file = Path::new("/loader-test/one-signature.jsonl");
        let (parses, sources) = hooked(file, &[]);
        let paths = paths();

        let loads = (0..8).map(|_| sources.load(&paths, Source::Claude, file, "100:1"));
        let loaded = futures_util::future::join_all(loads).await;
        assert_eq!(parses.count(), 1, "eight callers, one parse");
        assert!(
            loaded.iter().all(|transcript| Arc::ptr_eq(transcript, &loaded[0])),
            "the same parse for all"
        );
        assert_eq!(made_by(&loaded[0]), "parse 1");
        assert!(!in_flight(file));

        let again = sources.load(&paths, Source::Claude, file, "100:1").await;
        assert!(Arc::ptr_eq(&again, &loaded[0]), "and remembered, as before");
        assert_eq!(parses.count(), 1);
    }

    /// Two versions asked for at once are two parses, and each caller holds
    /// the one made for the signature it asked at, never the other.
    #[tokio::test(start_paused = true)]
    async fn each_signature_has_its_own_parse() {
        let file = Path::new("/loader-test/two-signatures.jsonl");
        let (parses, sources) = hooked(file, &[]);
        let paths = paths();

        let sigs = ["100:1", "200:2", "100:1", "200:2", "200:2", "100:1"];
        let loads = sigs.map(|sig| sources.load(&paths, Source::Claude, file, sig));
        let loaded = futures_util::future::join_all(loads).await;
        assert_eq!(parses.count(), 2, "one parse per signature");
        let (older, newer) = (&loaded[0], &loaded[1]);
        assert!(!Arc::ptr_eq(older, newer));
        for (sig, transcript) in sigs.iter().zip(&loaded) {
            let own = if *sig == "100:1" { older } else { newer };
            assert!(Arc::ptr_eq(transcript, own), "a caller at {sig} holds its own signature's parse");
        }
        assert_eq!(made_by(older), "parse 1", "the first asked for is the first parsed");
        assert!(!in_flight(file));
    }

    /// An older version's parse that finishes after a newer one's is handed
    /// to its own callers, but does not push the newer parse out: that stays
    /// the last good parse, and the one a caller at its signature finds.
    #[tokio::test(start_paused = true)]
    async fn an_older_parse_finishing_last_does_not_replace_a_newer_one() {
        let file = Path::new("/loader-test/older-finishes-last.jsonl");
        let (parses, sources) = hooked(file, &[3 * PARSE_TIME, PARSE_TIME]);
        let paths = paths();

        let (older, newer) = tokio::join!(
            sources.load(&paths, Source::Claude, file, "100:1"),
            sources.load(&paths, Source::Claude, file, "200:2"),
        );
        assert_eq!((made_by(&older), made_by(&newer)), ("parse 1", "parse 2"));
        let kept = cached_transcript(file, None).expect("a parse is remembered");
        assert!(Arc::ptr_eq(&kept, &newer), "the newer parse is the last good one");
        let again = sources.load(&paths, Source::Claude, file, "200:2").await;
        assert!(Arc::ptr_eq(&again, &newer));
        assert_eq!(parses.count(), 2, "and is not parsed again");
    }

    /// The same when the older parse finishes last because it was started
    /// again: a waiter re-running it, its first caller gone, takes the place
    /// the first caller took, before the newer parse started.
    #[tokio::test(start_paused = true)]
    async fn an_older_parse_started_again_does_not_replace_a_newer_one() {
        let file = Path::new("/loader-test/older-started-again.jsonl");
        // The first parse is abandoned at a fifth of PARSE_TIME, the newer
        // one starts at a tenth and ends at 2.1, the re-run ends at 4.2.
        let (parses, sources) = hooked(file, &[PARSE_TIME, 2 * PARSE_TIME, 4 * PARSE_TIME]);
        let paths = paths();

        let (abandoned, older, newer) = tokio::join!(
            tokio::time::timeout(PARSE_TIME / 5, sources.load(&paths, Source::Claude, file, "100:1")),
            sources.load(&paths, Source::Claude, file, "100:1"),
            async {
                tokio::time::sleep(PARSE_TIME / 10).await;
                sources.load(&paths, Source::Claude, file, "200:2").await
            },
        );
        assert!(abandoned.is_err(), "the first caller gave up");
        assert_eq!((made_by(&older), made_by(&newer)), ("parse 3", "parse 2"));
        let kept = cached_transcript(file, None).expect("a parse is remembered");
        assert!(Arc::ptr_eq(&kept, &newer), "the newer parse is the last good one");
        sources.load(&paths, Source::Claude, file, "200:2").await;
        assert_eq!(parses.count(), 3, "and is not parsed again");
    }

    /// A parse that fails is shared too: it is tried once for everyone
    /// waiting, and each of them gets the last good parse, as a lone caller
    /// always has. The next caller at that signature tries again.
    #[tokio::test(start_paused = true)]
    async fn a_failed_parse_is_shared_and_falls_back_to_the_last_good_one() {
        let file = Path::new("/loader-test/failed.jsonl");
        let (parses, sources) = hooked(file, &[]);
        let paths = paths();
        let good = sources.load(&paths, Source::Claude, file, "100:1").await;

        parses.failing.store(true, SeqCst);
        let loads = (0..4).map(|_| sources.load(&paths, Source::Claude, file, "200:2"));
        let loaded = futures_util::future::join_all(loads).await;
        assert_eq!(parses.count(), 2, "the good parse, and one failed one for all four");
        assert!(
            loaded.iter().all(|transcript| Arc::ptr_eq(transcript, &good)),
            "the last good parse for all"
        );
        assert!(!in_flight(file));

        sources.load(&paths, Source::Claude, file, "200:2").await;
        assert_eq!(parses.count(), 3, "a failure is not remembered");

        forget_transcript(file);
        let nothing = sources.load(&paths, Source::Claude, file, "200:2").await;
        assert!(nothing.is_empty(), "with nothing good to fall back on, no items");
    }

    /// What a failed read answers says it is not the version asked for, so
    /// a reader that remembers versions asks again; each ask is one read,
    /// however long the file stays unreadable. The first that reads it is
    /// the parse at that signature.
    #[tokio::test(start_paused = true)]
    async fn a_fallback_says_it_is_not_the_version_asked_for() {
        let file = Path::new("/loader-test/checked.jsonl");
        let (parses, sources) = hooked(file, &[]);
        let paths = paths();
        let (good, current) = sources.load_checked(&paths, Source::Claude, file, "100:1").await;
        assert!(current, "a parse at the signature asked for");

        parses.failing.store(true, SeqCst);
        for ask in 1..=3 {
            let (answer, current) = sources.load_checked(&paths, Source::Claude, file, "200:2").await;
            assert!(Arc::ptr_eq(&answer, &good), "ask {ask}: the last good parse");
            assert!(!current, "ask {ask}: said to be another version");
            assert_eq!(parses.count(), 1 + ask, "ask {ask}: one read each");
        }
        let (kept, current) = sources.load_checked(&paths, Source::Claude, file, "100:1").await;
        assert!(Arc::ptr_eq(&kept, &good) && current, "asked for by its own signature, it is that version");

        parses.failing.store(false, SeqCst);
        let (fresh, current) = sources.load_checked(&paths, Source::Claude, file, "200:2").await;
        assert!(current);
        assert_eq!(made_by(&fresh), "parse 5");

        forget_transcript(file);
        parses.failing.store(true, SeqCst);
        let (nothing, current) = sources.load_checked(&paths, Source::Claude, file, "300:3").await;
        assert!(nothing.is_empty() && !current, "and no items is no version either");
    }

    /// Counts the warnings logged on the thread it is set on.
    struct Warnings(Arc<AtomicUsize>);

    impl tracing::Subscriber for Warnings {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            if *event.metadata().level() == tracing::Level::WARN {
                self.0.fetch_add(1, SeqCst);
            }
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    /// A read that keeps failing at one signature is warned about once, and
    /// logged at debug level after that. Another signature is warned about
    /// again, and so is an old one once a read worked in between.
    #[tokio::test(start_paused = true)]
    async fn a_read_that_keeps_failing_is_warned_about_once_per_version() {
        let warnings = Arc::new(AtomicUsize::new(0));
        let _counting = tracing::subscriber::set_default(Warnings(warnings.clone()));
        let file = Path::new("/loader-test/warned.jsonl");
        let (parses, sources) = hooked(file, &[]);
        let paths = paths();

        parses.failing.store(true, SeqCst);
        for _ in 0..3 {
            sources.load_checked(&paths, Source::Claude, file, "100:1").await;
        }
        assert_eq!((parses.count(), warnings.load(SeqCst)), (3, 1), "three reads, one warning");
        sources.load_checked(&paths, Source::Claude, file, "200:2").await;
        assert_eq!(warnings.load(SeqCst), 2, "a new version is warned about");

        parses.failing.store(false, SeqCst);
        sources.load_checked(&paths, Source::Claude, file, "300:3").await;
        parses.failing.store(true, SeqCst);
        sources.load_checked(&paths, Source::Claude, file, "200:2").await;
        assert_eq!(
            (parses.count(), warnings.load(SeqCst)),
            (6, 3),
            "and an old one again, after a read that worked"
        );
    }

    /// The caller running the parse can go away mid-parse (a client that
    /// disconnected, a request that timed out): one still waiting runs it
    /// instead, rather than waiting for nothing — which would hang, so the
    /// test gives up well past when it should be done.
    #[tokio::test(start_paused = true)]
    async fn a_parse_whose_caller_went_away_is_run_by_one_still_waiting() {
        let file = Path::new("/loader-test/abandoned.jsonl");
        let (parses, sources) = hooked(file, &[]);
        let paths = paths();

        let both = async {
            tokio::join!(
                tokio::time::timeout(PARSE_TIME / 5, sources.load(&paths, Source::Claude, file, "100:1")),
                sources.load(&paths, Source::Claude, file, "100:1"),
            )
        };
        let (abandoned, waiter) = tokio::time::timeout(10 * PARSE_TIME, both)
            .await
            .expect("the waiter was not left waiting");
        assert!(abandoned.is_err(), "the first caller gave up");
        assert_eq!(parses.count(), 2, "the parse it left was run again");
        assert_eq!(made_by(&waiter), "parse 2");
        assert!(!in_flight(file));
    }

    /// Five items as a reader returns them, the first a user's.
    fn five_items() -> Vec<TranscriptItem> {
        (0..5)
            .map(|n| {
                let role = if n == 0 { Role::User } else { Role::Assistant };
                make_item(role, ItemKind::Text, &format!("item {n}"), Some("2026-11-20T00:00:00.000Z"))
            })
            .collect()
    }

    /// A prefix is made of the transcript's own items, not copies of them,
    /// and of its chain once that is made: the chain a parse of those items
    /// alone would have.
    #[test]
    fn a_prefix_shares_the_items_and_the_chain() {
        let file = Path::new("/loader-test/prefix.jsonl");
        let whole = Transcript::parsed(file, five_items());
        let fresh = fingerprint_chain(file, &whole.items[..3]);

        let early = whole.prefix(3);
        assert!(
            early.items.iter().zip(whole.items.iter()).all(|(mine, theirs)| Arc::ptr_eq(mine, theirs)),
            "the same items, not copies"
        );
        assert!(early.chain.get().is_none(), "no chain to share before one is made");
        assert_eq!(&early.chain()[..], fresh.as_slice());

        whole.chain();
        let late = whole.prefix(3);
        assert!(late.chain.get().is_some(), "the whole one's chain, cut");
        assert_eq!(&late.chain()[..], fresh.as_slice());
        assert_eq!(&whole.prefix(0).chain()[..], &fresh[..1], "an empty prefix is the seed alone");
    }

    /// The items of a parse hold one pointer each and no room for more: the
    /// reader's own buffer, sized for the items themselves, is not kept.
    #[test]
    fn a_parse_keeps_no_room_it_does_not_use() {
        let transcript = Transcript::parsed(Path::new("/loader-test/room.jsonl"), five_items());
        assert_eq!(transcript.items.capacity(), transcript.len());
    }

    /// An item behind its own `Arc` is written as the item itself: the sharing
    /// never reaches the wire.
    #[test]
    fn shared_items_are_written_as_the_items_themselves() {
        let items = five_items();
        let transcript = Transcript::parsed(Path::new("/loader-test/json.jsonl"), items.clone());
        assert_eq!(
            serde_json::to_string(&transcript.items[1..4]).expect("json"),
            serde_json::to_string(&items[1..4]).expect("json")
        );
    }

    /// A transcript on disk, read through a reader that goes on from the last
    /// parse ([`LinesReader`]), every parse of it keeping its checkpoint.
    struct Live {
        _dir: tempfile::TempDir,
        path: PathBuf,
        sources: TranscriptSources,
        paths: Paths,
    }

    impl Live {
        fn new(lines: &str) -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("live.jsonl");
            std::fs::write(&path, lines).expect("write");
            resume_from(&path, 0);
            Self {
                paths: Paths::new(dir.path().to_path_buf()),
                _dir: dir,
                path,
                sources: LinesReader::sources(),
            }
        }

        fn append(&self, text: &str) {
            let mut file = std::fs::OpenOptions::new().append(true).open(&self.path).expect("open");
            std::io::Write::write_all(&mut file, text.as_bytes()).expect("append");
        }

        async fn sig(&self) -> String {
            crate::file_signature(&self.path).await.expect("a signature")
        }

        async fn load(&self) -> Arc<Transcript> {
            self.load_checked().await.0
        }

        async fn load_checked(&self) -> (Arc<Transcript>, bool) {
            let sig = self.sig().await;
            self.sources.load_checked(&self.paths, Source::Claude, &self.path, &sig).await
        }

        fn reads(&self) -> Reads {
            reads_of(&self.path)
        }

        /// Wait, a few seconds at most, for the reads to come to `done`: a
        /// check runs in the background.
        async fn until(&self, done: impl Fn(&Reads) -> bool) {
            for _ in 0..500 {
                if done(&self.reads()) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("still {:?}", self.reads());
        }

        /// A parse of the whole file as it is now, past the cache.
        async fn whole(&self) -> Transcript {
            self.sources.read_uncached(&self.paths, Source::Claude, &self.path).await.expect("readable")
        }
    }

    fn texts(transcript: &Transcript) -> Vec<String> {
        transcript.items.iter().map(|item| item.text.clone()).collect()
    }

    /// The same items, as written, and the same fingerprints.
    fn assert_same(ours: &Transcript, whole: &Transcript) {
        assert_eq!(texts(ours), texts(whole));
        assert_eq!(ours.chain(), whole.chain(), "the fingerprints");
    }

    /// A transcript that grew is read on from its last parse: only the
    /// lines appended, and the fingerprint chain made on from the last one's.
    /// An item it changes moves the chain from there, and no further back.
    #[tokio::test]
    async fn a_grown_transcript_is_read_on_from_its_last_parse() {
        let live = Live::new("a\nb\n");
        check_resumes(&live.path);
        let first = live.load().await;
        first.chain();
        live.append("c\nanswer 1\n");
        let grown = live.load().await;
        assert_eq!(texts(&grown), ["a", "b (answered)", "c", "answer 1"]);
        assert!(Arc::ptr_eq(&grown.items[0], &first.items[0]), "an item it did not change is the same one");
        assert_eq!(&grown.chain()[..2], &first.chain()[..2], "the chain stands up to the item it changed");
        assert_same(&grown, &live.whole().await);
        assert_eq!((live.reads().full, live.reads().resumed), (1, 1));
        live.until(|reads| reads.checked == 1).await;

        // And again, from a parse that went on itself.
        live.append("d\n");
        assert_same(&*live.load().await, &live.whole().await);
        assert_eq!(live.reads().resumed, 2);
        live.until(|reads| reads.checked == 2).await;
        assert_eq!(live.reads().mismatched, 0, "each checked against the whole file, and agreeing");
    }

    /// However many callers ask for a grown version at once, it is read on
    /// once, and they all hold that one parse.
    #[tokio::test]
    async fn callers_at_one_signature_share_one_read_on() {
        let live = Live::new("a\n");
        live.load().await;
        live.append("b\n");
        let sig = live.sig().await;
        let loads = (0..8).map(|_| live.sources.load(&live.paths, Source::Claude, &live.path, &sig));
        let loaded = futures_util::future::join_all(loads).await;
        assert!(loaded.iter().all(|transcript| Arc::ptr_eq(transcript, &loaded[0])));
        assert_eq!((live.reads().full, live.reads().resumed), (1, 1));
    }

    /// Two newer versions asked for at once: the one that comes second waits
    /// for the one going on from the last parse, and goes on from what that
    /// read, rather than read the file whole for want of a checkpoint. The
    /// one that started later is the one remembered.
    #[tokio::test]
    async fn a_version_asked_for_while_another_goes_on_goes_on_from_it() {
        let live = Live::new("a\n");
        live.load().await;
        live.append("b\n");
        let middle = live.sig().await;
        live.append("c\n");
        let newer = live.sig().await;
        let gate = hold(&live.path, "reopen");
        let (one, two, ()) = tokio::join!(
            live.sources.load(&live.paths, Source::Claude, &live.path, &middle),
            live.sources.load(&live.paths, Source::Claude, &live.path, &newer),
            async {
                gate.reached().await;
                gate.open();
                gate.reached().await;
                gate.open();
            },
        );
        assert_eq!((texts(&one), texts(&two)), (vec!["a".to_string(), "b".into(), "c".into()], vec!["a".to_string(), "b".into(), "c".into()]));
        let (sig, kept) = live.sources.newest_cached(&live.path).expect("cached");
        assert_eq!(sig, newer, "the newer version, which started later");
        assert!(Arc::ptr_eq(&kept, &two));
        assert_eq!((live.reads().full, live.reads().resumed, live.reads().fallback), (1, 2, 0), "neither read whole");
    }

    /// A checkpoint taken by a parse whose read fails goes back on the parse
    /// it was taken off, and on no other. Here a newer parse is remembered
    /// while the read is held: put back on that one, the next parse would go
    /// on from where the older one stopped, over the newer one's items, and
    /// read what it holds again.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_checkpoint_goes_back_only_on_the_parse_it_was_taken_off() {
        use std::os::unix::fs::PermissionsExt;
        let live = Live::new("a\n");
        live.load().await;
        live.append("b\n");
        let sig = live.sig().await;
        let gate = hold(&live.path, "reopen");
        let ((_, current), ()) = tokio::join!(live.sources.load_checked(&live.paths, Source::Claude, &live.path, &sig), async {
            gate.reached().await;
            live.append("c\n");
            let newer = Transcript::parsed(&live.path, ["a", "b", "c"].map(|text| make_item(Role::User, ItemKind::Text, text, None)).into());
            let started = PARSES_STARTED.fetch_add(1, Ordering::Relaxed);
            remember_transcript(&live.path, &live.sig().await, started, Arc::new(newer), None);
            std::fs::set_permissions(&live.path, std::fs::Permissions::from_mode(0o000)).expect("chmod");
            gate.open();
        });
        assert!(!current, "the read failed");
        std::fs::set_permissions(&live.path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        // A parse that would go on (wrongly) is let through, to be caught.
        gate.open();
        live.append("d\n");
        assert_same(&*live.load().await, &live.whole().await);
        assert_eq!(live.reads().resumed, 0, "nothing to go on from on the newer parse");
    }

    /// A parse that goes on and finishes after a newer parse was remembered
    /// is its caller's, and the newer one stays the one remembered.
    #[tokio::test]
    async fn a_parse_going_on_that_ends_after_a_newer_one_leaves_the_newer_one() {
        let live = Live::new("a\n");
        live.load().await;
        live.append("b\n");
        let sig = live.sig().await;
        let gate = hold(&live.path, "reopen");
        let newer = Arc::new(Transcript::parsed(&live.path, vec![make_item(Role::User, ItemKind::Text, "newer", None)]));
        let ((read, current), ()) = tokio::join!(live.sources.load_checked(&live.paths, Source::Claude, &live.path, &sig), async {
            gate.reached().await;
            let started = PARSES_STARTED.fetch_add(1, Ordering::Relaxed);
            remember_transcript(&live.path, "999999:9", started, newer.clone(), None);
            gate.open();
        });
        assert!(current);
        assert_eq!(texts(&read), ["a", "b"]);
        let (sig, kept) = live.sources.newest_cached(&live.path).expect("cached");
        assert_eq!(sig, "999999:9");
        assert!(Arc::ptr_eq(&kept, &newer), "the newer one stays");
    }

    /// The caller of a parse going on can go away mid-read; one waiting at
    /// the same signature runs it again, and goes on from the same parse,
    /// whose checkpoint the first put back.
    #[tokio::test]
    async fn a_parse_going_on_whose_caller_went_away_is_run_again_from_the_same_checkpoint() {
        let live = Live::new("a\n");
        live.load().await;
        live.append("b\n");
        let sig = live.sig().await;
        let gate = hold(&live.path, "reopen");
        let (abandoned, waiter, ()) = tokio::join!(
            tokio::time::timeout(Duration::from_millis(100), live.sources.load(&live.paths, Source::Claude, &live.path, &sig)),
            async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                live.sources.load(&live.paths, Source::Claude, &live.path, &sig).await
            },
            async {
                gate.reached().await;
                gate.reached().await;
                gate.open();
            },
        );
        assert!(abandoned.is_err(), "the first caller went away");
        assert_eq!(texts(&waiter), ["a", "b"]);
        assert_eq!((live.reads().full, live.reads().resumed), (1, 1), "gone on from the first parse");
    }

    /// A parse pushed out of the cache while another goes on from it is
    /// still gone on from, and what that read is remembered and gone on from
    /// next.
    #[tokio::test]
    async fn a_parse_pushed_out_while_another_goes_on_from_it_is_still_gone_on_from() {
        let live = Live::new("a\n");
        live.load().await;
        live.append("b\n");
        let gate = hold(&live.path, "reopen");
        let others: Vec<Live> = (0..TRANSCRIPT_CACHE_MAX).map(|_| Live::new("x\n")).collect();
        let (read, ()) = tokio::join!(live.load(), async {
            gate.reached().await;
            for other in &others {
                other.load().await;
            }
            assert!(live.sources.newest_cached(&live.path).is_none(), "pushed out");
            gate.open();
        });
        assert_eq!(texts(&read), ["a", "b"]);
        live.append("c\n");
        gate.open();
        assert_same(&*live.load().await, &live.whole().await);
        assert_eq!((live.reads().full, live.reads().resumed), (1, 2));
    }

    /// More appended than a parse reads on is read whole.
    #[tokio::test]
    async fn more_appended_than_a_parse_reads_on_is_read_whole() {
        let live = Live::new("a\n");
        read_on_at_most(&live.path, 4);
        live.load().await;
        live.append("bb\n");
        live.load().await;
        live.append("cccccc\n");
        assert_same(&*live.load().await, &live.whole().await);
        assert_eq!((live.reads().full, live.reads().resumed, live.reads().fallback), (1, 1, 1));
    }

    /// Once the last whole read is old enough, the next parse reads the file
    /// whole, however few went on since.
    #[tokio::test]
    async fn a_file_last_read_whole_long_enough_ago_is_read_whole() {
        let live = Live::new("a\n");
        read_whole_every(&live.path, Duration::from_millis(200));
        live.load().await;
        live.append("b\n");
        live.load().await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        live.append("c\n");
        assert_same(&*live.load().await, &live.whole().await);
        assert_eq!((live.reads().full, live.reads().resumed), (2, 1));
        live.append("d\n");
        live.load().await;
        assert_eq!(live.reads().resumed, 2, "and from there it goes on again");
    }

    /// The check of a parse that went on runs one at a time per transcript:
    /// one that goes on while a check runs goes unchecked. And a check whose
    /// reader's other inputs moved meanwhile says nothing, whatever it finds,
    /// and the checkpoint stays.
    #[tokio::test]
    async fn checks_run_one_at_a_time_and_say_nothing_when_other_inputs_moved() {
        let live = Live::new("a\n");
        check_resumes(&live.path);
        lie(&live.path);
        let gate = hold(&live.path, "check");
        live.load().await;
        live.append("b\n");
        live.load().await;
        gate.reached().await;
        live.append("c\n");
        live.load().await;
        assert_eq!(live.reads().skipped, 1, "one check at a time");
        move_inputs(&live.path);
        gate.open();
        live.until(|reads| reads.inconclusive == 1).await;
        assert_eq!((live.reads().checked, live.reads().mismatched), (0, 0), "a lie it cannot tell from a moved input");
        live.append("d\n");
        live.load().await;
        assert_eq!(live.reads().resumed, 3, "the checkpoint stayed");
        gate.open();
        live.until(|reads| reads.checked == 1).await;
        assert_eq!(live.reads().mismatched, 1, "and with nothing moving, the lie is caught");
    }

    /// A parse pushed out of the cache takes its checkpoint with it: the next
    /// version is read whole.
    #[tokio::test]
    async fn a_parse_pushed_out_of_the_cache_is_not_gone_on_from() {
        let live = Live::new("a\n");
        live.load().await;
        let others: Vec<Live> = (0..TRANSCRIPT_CACHE_MAX).map(|_| Live::new("x\n")).collect();
        for other in &others {
            other.load().await;
        }
        live.append("b\n");
        assert_same(&*live.load().await, &live.whole().await);
        assert_eq!((live.reads().full, live.reads().resumed, live.reads().fallback), (2, 0, 0));
    }

    /// A version that cannot be read is a failed read, as ever, and the
    /// parse it would have gone on from keeps its checkpoint: once the file
    /// reads again, the next try goes on from it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_read_that_fails_leaves_the_checkpoint_for_the_next_try() {
        use std::os::unix::fs::PermissionsExt;
        let live = Live::new("a\n");
        let first = live.load().await;
        live.append("b\n");
        std::fs::set_permissions(&live.path, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        let (fallback, current) = live.load_checked().await;
        assert!(!current && Arc::ptr_eq(&fallback, &first), "the last good parse, said not to be the version asked for");
        std::fs::set_permissions(&live.path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let (read, current) = live.load_checked().await;
        assert!(current);
        assert_eq!(texts(&read), ["a", "b"]);
        assert_eq!((live.reads().full, live.reads().resumed), (1, 1), "the retry went on from the first parse");
    }

    /// After [`RESUMES_BEFORE_WHOLE`] parses in a row that went on, the next
    /// one reads the file whole, and the ones after it go on again.
    #[tokio::test]
    async fn every_so_many_reads_on_the_file_is_read_whole() {
        let live = Live::new("start\n");
        live.load().await;
        for n in 0..RESUMES_BEFORE_WHOLE {
            live.append(&format!("{n}\n"));
            live.load().await;
        }
        assert_eq!((live.reads().full, live.reads().resumed), (1, RESUMES_BEFORE_WHOLE));
        live.append("whole\n");
        assert_same(&*live.load().await, &live.whole().await);
        assert_eq!((live.reads().full, live.reads().resumed), (2, RESUMES_BEFORE_WHOLE));
        live.append("on\n");
        live.load().await;
        assert_eq!(live.reads().resumed, RESUMES_BEFORE_WHOLE + 1);
    }

    /// A reader that will not go on from a parse (a chat's delivery rows
    /// moved) has the file read whole, and that is no failure.
    #[tokio::test]
    async fn a_reader_that_will_not_go_on_has_the_file_read_whole() {
        let live = Live::new("a\n");
        live.load().await;
        go_stale(&live.path);
        live.append("b\n");
        let (read, current) = live.load_checked().await;
        assert!(current);
        assert_same(&read, &live.whole().await);
        assert_eq!((live.reads().resumed, live.reads().fallback), (0, 1));
    }

    /// A transcript rewritten rather than grown is read whole: one whose
    /// bytes before the stop changed; and one that kept both ends of what
    /// was read and changed its middle, which only its signature can tell,
    /// when it went back, as a copy put back with its old mtime does.
    #[tokio::test]
    async fn a_rewritten_transcript_is_read_whole() {
        let live = Live::new("a\nb\n");
        live.load().await;
        std::fs::write(&live.path, "a\nB\nc\n").expect("rewrite");
        assert_eq!(texts(&*live.load().await), ["a", "B", "c"]);
        assert_eq!((live.reads().resumed, live.reads().fallback), (0, 1), "the bytes before the stop moved");

        let live = Live::new("head\nm\nb\nc\n");
        live.load().await;
        let before = std::fs::metadata(&live.path).expect("stat").modified().expect("mtime");
        std::fs::write(&live.path, "head\nM\nb\nc\nd\n").expect("the middle rewritten, and grown");
        let file = std::fs::OpenOptions::new().write(true).open(&live.path).expect("open");
        file.set_modified(before - Duration::from_secs(60)).expect("an older mtime");
        assert_eq!(texts(&*live.load().await), ["head", "M", "b", "c", "d"]);
        assert_eq!((live.reads().resumed, live.reads().fallback), (0, 1), "an older file is not the last one grown");
        live.append("e\n");
        assert_same(&*live.load().await, &live.whole().await);
        assert_eq!(live.reads().resumed, 1, "on from the parse that read it whole");
    }

    /// A parse that went on and does not agree with the whole file's is
    /// caught by the check, which drops its checkpoint: the next version is
    /// read whole.
    #[tokio::test]
    async fn a_read_on_the_check_finds_wrong_is_not_gone_on_from() {
        let live = Live::new("a\n");
        check_resumes(&live.path);
        lie(&live.path);
        live.load().await;
        live.append("b\n");
        live.load().await;
        live.until(|reads| reads.checked == 1).await;
        assert_eq!(live.reads().mismatched, 1);
        live.append("c\n");
        assert_eq!(texts(&*live.load().await), ["a", "b", "c"], "read whole, the lie gone");
        assert_eq!((live.reads().full, live.reads().resumed), (2, 1));
    }

    /// A transcript read compressed goes on in the plain file written back
    /// and grown, as Codex writes it back before it appends.
    #[tokio::test]
    async fn a_transcript_read_compressed_goes_on_in_the_plain_one_written_back() {
        let live = Live::new("");
        std::fs::remove_file(&live.path).expect("plain gone");
        let zst = crate::compressed::compressed_path(&live.path);
        std::fs::write(&zst, crate::compressed::compress(b"a\nb\n")).expect("compressed");
        assert_eq!(texts(&*live.load().await), ["a", "b"]);
        std::fs::write(&live.path, "a\nb\nc\n").expect("written back, grown");
        std::fs::remove_file(&zst).expect("compressed gone");
        let grown = live.load().await;
        assert_same(&grown, &live.whole().await);
        assert_eq!((live.reads().full, live.reads().resumed), (1, 1));
    }

    /// A short transcript keeps no checkpoint ([`RESUME_MIN_BYTES`]), and a
    /// reader that cannot go on reads every version whole.
    #[tokio::test]
    async fn a_short_transcript_is_read_whole_every_time() {
        let live = Live::new("a\n");
        resume_from(&live.path, RESUME_MIN_BYTES);
        live.load().await;
        live.append("b\n");
        live.load().await;
        assert_eq!((live.reads().full, live.reads().resumed), (2, 0));
    }
}
