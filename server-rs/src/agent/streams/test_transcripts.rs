//! Transcripts and session rows for the streams' tests, made the way the
//! server's session loader makes them, so the tests do not need the loader:
//! the streams read a transcript only through [`Transcript`], [`Items`] and
//! [`Fingerprints`], and a row only through [`SessionRow`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::source::{Fingerprints, Items, SessionRow, Transcript};

/// As many items before the end as a resume re-sends: the session loader's
/// `RESUME_OVERLAP`.
pub(in crate::agent) const RESUME_OVERLAP: usize = 3;

/// One link of a fingerprint chain: 128 bits of SHA-256, as the loader keeps.
pub(in crate::agent) type Fp = [u8; 16];

/// A transcript's items, each the JSON a transcript item writes.
pub(in crate::agent) struct TestItems(pub(in crate::agent) Vec<Value>);

impl Items for TestItems {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn json(&self, i: usize) -> Value {
        self.0[i].clone()
    }
    fn size(&self, i: usize) -> usize {
        serde_json::to_vec(&self.0[i]).map_or(0, |b| b.len())
    }
    fn timestamp(&self, i: usize) -> Option<&str> {
        self.0[i]["timestamp"].as_str()
    }
}

/// A transcript's fingerprint chain, made as the loader makes it: entry `i`
/// covers items `[0, i)` of one file, so there are `items + 1` entries, the
/// same file with the same items has the same chain, and a change to item
/// `i` changes every entry after `i`.
pub(in crate::agent) struct TestChain(pub(in crate::agent) Vec<Fp>);

impl Fingerprints for TestChain {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn at(&self, at: usize) -> Option<String> {
        self.0.get(at).map(encode_fp)
    }
    fn resume(&self, end: usize) -> Option<String> {
        self.0.get(end.saturating_sub(RESUME_OVERLAP)).map(encode_fp)
    }
}

/// How a test spells a fingerprint.
pub(in crate::agent) fn encode_fp(fp: &Fp) -> String {
    fp.iter().map(|b| format!("{b:02x}")).collect()
}

/// The fingerprint chain of `items` read from `path`.
pub(in crate::agent) fn fingerprint_chain(path: &Path, items: &TestItems) -> TestChain {
    let cut = |digest: &[u8]| {
        let mut fp = [0u8; 16];
        fp.copy_from_slice(&digest[..16]);
        fp
    };
    let mut seed = Sha256::new();
    seed.update(b"test transcript fingerprint\0");
    seed.update(path.as_os_str().as_encoded_bytes());
    let mut link = cut(&seed.finalize());
    let mut chain = vec![link];
    for item in &items.0 {
        let mut hasher = Sha256::new();
        hasher.update(link);
        hasher.update(serde_json::to_vec(item).unwrap_or_default());
        link = cut(&hasher.finalize());
        chain.push(link);
    }
    TestChain(chain)
}

/// One parse of a transcript file: its items and their chain.
pub(in crate::agent) struct TestTranscript {
    path: PathBuf,
    pub(in crate::agent) items: TestItems,
    pub(in crate::agent) chain: TestChain,
}

impl TestTranscript {
    /// `items` as read from `path`.
    pub(in crate::agent) fn new(path: &Path, items: TestItems) -> Self {
        let chain = fingerprint_chain(path, &items);
        Self {
            path: path.to_path_buf(),
            items,
            chain,
        }
    }
}

impl Transcript for TestTranscript {
    fn items(&self) -> &dyn Items {
        &self.items
    }
    fn chain(&self) -> &dyn Fingerprints {
        &self.chain
    }
    fn prefix(&self, end: usize) -> Arc<dyn Transcript> {
        Arc::new(Self::new(&self.path, TestItems(self.items.0[..end].to_vec())))
    }
}

/// A row of the session list, as the session list serves it.
pub(in crate::agent) struct TestRow {
    source: String,
    id: String,
    updated_at: String,
    owned: bool,
    json: Value,
}

impl TestRow {
    /// The row `json`, the session list's JSON of one session.
    pub(in crate::agent) fn of(json: Value) -> Self {
        let text = |key: &str| json[key].as_str().unwrap_or_default().to_string();
        Self {
            source: text("source"),
            id: text("id"),
            updated_at: text("updatedAt"),
            owned: json["scope"] == "owned",
            json,
        }
    }
}

impl SessionRow for TestRow {
    fn source(&self) -> &str {
        &self.source
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn updated_at(&self) -> &str {
        &self.updated_at
    }
    fn owned(&self) -> bool {
        self.owned
    }
    fn json(&self) -> Option<String> {
        serde_json::to_string(&self.json).ok()
    }
}
