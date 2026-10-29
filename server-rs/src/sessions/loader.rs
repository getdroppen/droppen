//! One parse of a session's transcript, with the fingerprint of every prefix
//! of it, and the small cache of the last parses that every reader shares.
//!
//! It reads files and holds no live state, so it sits with the transcript
//! parsers: the live feed, the history routes and the cloud agent's streams
//! all read through it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use sha2::{Digest, Sha256};

use super::util::TranscriptItem;
use super::{Source, TranscriptSources};
use droppen_instance::paths::Paths;

/// How many of a client's newest items a resume re-sends: `K` in
/// [`crate::feed`]'s module docs. Those are the items the parser may still
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
///   change is what the TypeScript's `cachedTranscript` bought too.
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
#[cfg(test)]
pub(crate) fn forget_transcript(file_path: &Path) {
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
    pub async fn load(&self, paths: &Paths, source: Source, file_path: &Path, sig: &str) -> Arc<Transcript> {
        if let Some(transcript) = cached_transcript(file_path, Some(sig)) {
            return transcript;
        }
        let parsed = self
            .parse(paths, source, file_path)
            .await
            .map(|items| Transcript::new(file_path, Arc::new(items)));
        match parsed {
            Ok(transcript) => {
                let transcript = Arc::new(transcript);
                remember_transcript(file_path, sig, transcript.clone());
                transcript
            }
            Err(error) => {
                tracing::warn!(path = %file_path.display(), %error, "cannot read transcript; keeping the last good items");
                cached_transcript(file_path, None).unwrap_or_else(|| Arc::new(Transcript::new(file_path, Arc::default())))
            }
        }
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
