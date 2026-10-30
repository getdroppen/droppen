//! The rest of an encrypted stream (architecture doc §5, "加密流"): what each
//! stream a machine writes holds, the snapshot pages and signed manifests old
//! records are folded into, and drafts that travel beside a stream without
//! ever being stored.
//!
//! # The streams a machine writes
//!
//! | stream | name (`source`, `id`) | record format | a record's body (JSON) |
//! |---|---|---|---|
//! | index | (`rh.index`, ``) | [`FMT_INDEX_V1`] | `{"reset"?: true, "upserts": [SessionSummary…], "removed": ["source/id"…], "asOf"}`, or just `{"asOf"}` (a daily write that changes nothing) |
//! | status | (`rh.status`, ``) | [`FMT_STATUS_V1`] | the whole machine status, replacing the last |
//! | session | (`claude`/`codex`, the CLI's session id) | [`FMT_FEED_V1`] | `{"fromIndex", "items", "endIndex", "fp", "prefixFp"}`, replace-from |
//!
//! A session copy the account's retention window trimmed (relay-and-plans
//! §3.4) starts its manifest with a *window-start page*: a session body with
//! no items at `base`, plus `"base": base, "before": "<RFC 3339>"` — the
//! copy holds items from `base` on, the earlier ones are only on the machine.
//! A reader that does not know it folds it as an empty replace-from at
//! `base`, i.e. a copy that starts there.
//!
//! Names are [`crate::record::stream_id`] under the machine key, so the hub
//! cannot tell an index from a session, nor a session from another machine's.
//! Which epoch's key: with [`STREAMS_FEATURE`] negotiated, the epoch a stream
//! was created or migrated in — the index and status always the newest, a
//! session's the index entry's `se` (absent: the first epoch, as every stream
//! was named before). A rotation moves the streams to new names, so a device
//! removed with the old key cannot compute the ones written after it.
//!
//! # Snapshot pages and manifests
//!
//! A stream's history is a manifest plus a tail of records. The manifest is
//! a [`SignedHead`] at `next_seq = N` (signed as any head is) and the hashes
//! of the pages, in order, all signed together by the machine
//! ([`sig::STREAM_MANIFEST`]). The pages hold the stream's state at `N` as a
//! run of bodies of the stream's own format: applying them in order to an
//! empty state, then the records from `N` on, gives the current state. A
//! reader starts from the manifest's head ([`reader_from_manifest`]) and
//! needs nothing older.
//!
//! A page is sealed under the stream key with its own AAD
//! (`"rh/v1 page\0" ‖ stream ‖ epoch`) and padded like a record. It is named
//! by the SHA-256 of its encoding — content-addressed, immutable — and the
//! hub checks that name when it is uploaded; a reader checks it again, and
//! the signed manifest is what makes the list of names trustworthy. Pages are
//! not bound to one manifest: a later manifest reuses the pages of an
//! unchanged prefix rather than uploading them again.
//!
//! A manifest whose head carries the link the stream's records reached
//! (compaction) lets a reader that already holds those records carry on as it
//! was. One whose link is fresh (a *rebase* — the agent rewriting the stream
//! from its own transcript, after losing track of what the hub holds, or in a
//! new epoch) breaks the chain on purpose: a reader holding older records
//! cannot extend them and starts again from the manifest.
//!
//! # Drafts
//!
//! A draft — the count of a think the agent is in the middle of (the server's
//! `provider/drafts.rs`), which no record can carry yet — is an `EPHEMERAL`
//! payload: sealed under the stream key with `"rh/v1 ephemeral\0" ‖ stream ‖
//! epoch` as its AAD, relayed to a stream's live readers and never stored.
//! The machine signs it ([`seal_ephemeral`]), so a holder of the stream key
//! that is not the machine cannot put one in its mouth.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use ed25519_dalek::SigningKey;
use rand_core::{CryptoRng, RngCore};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::codec::{DecodeError, Reader};
use crate::record::{self, bucket, Compression, LogReader, RecordError, SignedHead, StreamId, BUCKETS, FLAG_DEFLATE, MAX_BODY, MAX_CT, TAG_LEN};
pub use crate::record::{FMT_DRAFT_V1, FMT_FEED_V1, FMT_INDEX_V1, FMT_STATUS_V1};
use crate::sig;

/// What a device puts in its hello's client info (and an agent in its
/// features) when it reads streams named per key epoch (relay-and-plans §5):
/// the index and status under the newest epoch, a session's stream under the
/// epoch its index entry gives (`se`; none: the first). An agent names its
/// streams this way only once a device has said it reads them.
pub const STREAMS_FEATURE: &str = "streams/2";

/// Whether `info` (a hello's client info, space-separated tokens) names [`STREAMS_FEATURE`].
pub fn reads_epoch_names(info: &[u8]) -> bool {
    std::str::from_utf8(info).is_ok_and(|s| s.split_whitespace().any(|t| t == STREAMS_FEATURE))
}

/// The `source` of a machine's session-index stream (its `id` is empty).
pub const INDEX_SOURCE: &str = "rh.index";
/// The `source` of a machine's status stream (its `id` is empty).
pub const STATUS_SOURCE: &str = "rh.status";

/// The most pages one manifest may name: ~60 MiB of state at full pages,
/// and a manifest that still fits a `CHECKPOINT` frame.
pub const MAX_PAGES: usize = 1024;
/// The largest body a page (or a draft) can carry uncompressed.
pub const MAX_PAGE_BODY: usize = record::MAX_PLAINTEXT - PAGE_HEADER;
/// The largest encoded page: `epoch | nonce | ct_len | ct`.
pub const MAX_PAGE_LEN: usize = 4 + 12 + 4 + MAX_CT;

const PAGE_HEADER: usize = 1 + 2 + 4;
const PAGE_AAD: &[u8] = b"rh/v1 page\0";
const EPHEMERAL_AAD: &[u8] = b"rh/v1 ephemeral\0";

/// The index stream's name under `mk`.
pub fn index_stream(mk: &[u8; 32]) -> StreamId {
    record::stream_id(mk, INDEX_SOURCE, "").expect("a fixed, valid name")
}

/// The status stream's name under `mk`.
pub fn status_stream(mk: &[u8; 32]) -> StreamId {
    record::stream_id(mk, STATUS_SOURCE, "").expect("a fixed, valid name")
}

/// A snapshot page as the hub stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedPage {
    pub epoch: u32,
    pub nonce: [u8; 12],
    pub ct: Vec<u8>,
}

/// A page decrypted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedPage {
    pub fmt: u16,
    pub body: Vec<u8>,
}

impl SealedPage {
    /// `epoch u32 | nonce 12 | ct_len u32 | ct`.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(20 + self.ct.len());
        out.extend_from_slice(&self.epoch.to_be_bytes());
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&(self.ct.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.ct);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let epoch = r.u32()?;
        let nonce = r.array()?;
        let len = r.u32()? as usize;
        if len > MAX_CT {
            return Err(DecodeError::TooLarge {
                what: "page ciphertext",
                len,
                max: MAX_CT,
            });
        }
        if len < TAG_LEN {
            return Err(DecodeError::Invalid("page ciphertext shorter than its tag"));
        }
        let ct = r.bytes(len)?.to_vec();
        r.finish()?;
        Ok(Self { epoch, nonce, ct })
    }

    /// The page's name: the SHA-256 of its encoding.
    pub fn hash(&self) -> [u8; 32] {
        page_hash(&self.encode())
    }
}

/// The name of a page whose encoding is `bytes`.
pub fn page_hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn bound_aad(label: &[u8], stream: &StreamId, epoch: u32) -> Vec<u8> {
    let mut aad = Vec::with_capacity(label.len() + 32 + 4);
    aad.extend_from_slice(label);
    aad.extend_from_slice(&stream.0);
    aad.extend_from_slice(&epoch.to_be_bytes());
    aad
}

/// `[flags u8][fmt u16][body_len u32][body][zero padding]`, padded to a bucket.
fn pack(fmt: u16, body: &[u8], compression: Compression) -> Result<Zeroizing<Vec<u8>>, RecordError> {
    if body.len() > MAX_BODY {
        return Err(RecordError::TooLarge(body.len()));
    }
    let compressed = match compression {
        Compression::Deflate => record::deflate(body),
        Compression::None => None,
    };
    let (flags, stored) = match compressed {
        Some(ref c) if c.len() < body.len() => (FLAG_DEFLATE, c.as_slice()),
        _ => (0, body),
    };
    let padded = bucket(PAGE_HEADER + stored.len()).ok_or(RecordError::TooLarge(stored.len()))?;
    let mut pt = Zeroizing::new(Vec::with_capacity(padded));
    pt.push(flags);
    pt.extend_from_slice(&fmt.to_be_bytes());
    pt.extend_from_slice(&(stored.len() as u32).to_be_bytes());
    pt.extend_from_slice(stored);
    pt.resize(padded, 0);
    Ok(pt)
}

fn unpack(pt: &[u8]) -> Result<OpenedPage, RecordError> {
    if !BUCKETS.contains(&pt.len()) {
        return Err(RecordError::Malformed("plaintext is not a bucket size"));
    }
    let mut r = Reader::new(pt);
    let flags = r.u8()?;
    if flags & !FLAG_DEFLATE != 0 {
        return Err(RecordError::Malformed("unknown flags"));
    }
    let fmt = r.u16()?;
    let len = r.u32()? as usize;
    if len > r.remaining() {
        return Err(RecordError::Malformed("body longer than the page"));
    }
    let stored = r.bytes(len)?;
    if r.rest().iter().any(|&b| b != 0) {
        return Err(RecordError::Malformed("non-zero padding"));
    }
    let body = if flags & FLAG_DEFLATE != 0 {
        record::inflate(stored)?
    } else {
        stored.to_vec()
    };
    Ok(OpenedPage { fmt, body })
}

fn encrypt(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], pt: &[u8]) -> Result<Vec<u8>, RecordError> {
    Aes256Gcm::new_from_slice(key)
        .expect("32-byte key")
        .encrypt(Nonce::from_slice(nonce), Payload { msg: pt, aad })
        .map_err(|_| RecordError::Decrypt)
}

fn decrypt(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], ct: &[u8]) -> Result<Zeroizing<Vec<u8>>, RecordError> {
    Aes256Gcm::new_from_slice(key)
        .expect("32-byte key")
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad })
        .map(Zeroizing::new)
        .map_err(|_| RecordError::Decrypt)
}

/// Seal one snapshot page of stream `stream` in `epoch` (`key` is that
/// epoch's stream key). Compression follows the records' rule: off unless
/// the body mixes no attacker-influenced text with secrets.
pub fn seal_page<R: RngCore + CryptoRng>(
    key: &[u8; 32],
    stream: &StreamId,
    epoch: u32,
    fmt: u16,
    body: &[u8],
    compression: Compression,
    rng: &mut R,
) -> Result<SealedPage, RecordError> {
    let mut nonce = [0u8; 12];
    rng.fill_bytes(&mut nonce);
    seal_page_with_nonce(key, stream, epoch, fmt, body, compression, nonce)
}

pub(crate) fn seal_page_with_nonce(
    key: &[u8; 32],
    stream: &StreamId,
    epoch: u32,
    fmt: u16,
    body: &[u8],
    compression: Compression,
    nonce: [u8; 12],
) -> Result<SealedPage, RecordError> {
    let pt = pack(fmt, body, compression)?;
    let ct = encrypt(key, &nonce, &bound_aad(PAGE_AAD, stream, epoch), &pt)?;
    Ok(SealedPage { epoch, nonce, ct })
}

/// Decrypt one page. Whether it belongs where it was found is the manifest's
/// business ([`Manifest::pages`]): check the hash first.
pub fn open_page(key: &[u8; 32], stream: &StreamId, page: &SealedPage) -> Result<OpenedPage, RecordError> {
    let pt = decrypt(key, &page.nonce, &bound_aad(PAGE_AAD, stream, page.epoch), &page.ct)?;
    unpack(&pt)
}

/// A stream's checkpoint: a head the machine signed, and the pages holding
/// the stream's state at that head, signed together.
///
/// The pages' format is inside each page's ciphertext only: a manifest in the
/// clear says nothing about what kind of stream it is (the M2 review found a
/// cleartext `fmt` here told index, status and session streams apart).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub head: SignedHead,
    pub pages: Vec<[u8; 32]>,
    pub signature: [u8; 64],
}

impl Manifest {
    /// `signed head | count u16 | pages`: what the manifest signature covers.
    fn body(head: &SignedHead, pages: &[[u8; 32]]) -> Vec<u8> {
        let mut out = head.encode();
        out.extend_from_slice(&(pages.len() as u16).to_be_bytes());
        for page in pages {
            out.extend_from_slice(page);
        }
        out
    }

    /// Sign a manifest: `head` must already be signed by the same machine key.
    pub fn sign(head: SignedHead, pages: Vec<[u8; 32]>, machine: &SigningKey) -> Result<Self, RecordError> {
        if pages.len() > MAX_PAGES {
            return Err(RecordError::TooLarge(pages.len()));
        }
        let signature = sig::sign(machine, sig::STREAM_MANIFEST, &Self::body(&head, &pages));
        Ok(Self { head, pages, signature })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Self::body(&self.head, &self.pages);
        out.extend_from_slice(&self.signature);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        const HEAD: usize = 32 + 8 + 4 + 32 + 8 + 64;
        if bytes.len() < HEAD {
            return Err(DecodeError::Truncated);
        }
        let head = SignedHead::decode(&bytes[..HEAD])?;
        let mut r = Reader::new(&bytes[HEAD..]);
        let count = usize::from(r.u16()?);
        if count > MAX_PAGES {
            return Err(DecodeError::TooLarge {
                what: "manifest pages",
                len: count,
                max: MAX_PAGES,
            });
        }
        let mut pages = Vec::with_capacity(count);
        for _ in 0..count {
            pages.push(r.array()?);
        }
        let signature = r.array()?;
        r.finish()?;
        Ok(Self { head, pages, signature })
    }

    /// Both signatures are `machine_key`'s: the head's and the manifest's.
    pub fn verify(&self, machine_key: &[u8; 32]) -> bool {
        self.head.verify(machine_key) && sig::verify(machine_key, sig::STREAM_MANIFEST, &Self::body(&self.head, &self.pages), &self.signature)
    }

    pub fn stream(&self) -> StreamId {
        self.head.head.stream
    }

    /// The seq the manifest's state stands at: records from here on follow it.
    pub fn upto_seq(&self) -> u64 {
        self.head.head.next_seq
    }
}

/// A reader starting from a manifest (after checking both its signatures);
/// the pages, applied in order, are the state it starts with.
pub fn reader_from_manifest(machine_key: [u8; 32], manifest: &Manifest) -> Result<LogReader, RecordError> {
    if !manifest.verify(&machine_key) {
        return Err(RecordError::BadHead);
    }
    LogReader::from_head(machine_key, &manifest.head)
}

/// Seal a draft for a stream's live readers: `epoch u32 | nonce 12 | ct |
/// sig 64`, the machine's signature (`rh/v1 stream-draft`) over `stream |
/// epoch | nonce | ct` — so a device that holds the stream key (one removed
/// since, say) cannot put words in the machine's mouth.
pub fn seal_ephemeral<R: RngCore + CryptoRng>(
    key: &[u8; 32],
    stream: &StreamId,
    epoch: u32,
    body: &[u8],
    machine: &SigningKey,
    rng: &mut R,
) -> Result<Vec<u8>, RecordError> {
    let mut nonce = [0u8; 12];
    rng.fill_bytes(&mut nonce);
    seal_ephemeral_with_nonce(key, stream, epoch, body, machine, nonce)
}

fn draft_signed(stream: &StreamId, sealed: &[u8]) -> Vec<u8> {
    [&stream.0[..], sealed].concat()
}

pub(crate) fn seal_ephemeral_with_nonce(
    key: &[u8; 32],
    stream: &StreamId,
    epoch: u32,
    body: &[u8],
    machine: &SigningKey,
    nonce: [u8; 12],
) -> Result<Vec<u8>, RecordError> {
    let pt = pack(FMT_DRAFT_V1, body, Compression::None)?;
    let ct = encrypt(key, &nonce, &bound_aad(EPHEMERAL_AAD, stream, epoch), &pt)?;
    let mut out = Vec::with_capacity(16 + ct.len() + 64);
    out.extend_from_slice(&epoch.to_be_bytes());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    let signature = sig::sign(machine, sig::STREAM_DRAFT, &draft_signed(stream, &out));
    out.extend_from_slice(&signature);
    Ok(out)
}

/// Open a draft: `(epoch, body)`. It must carry `machine_key`'s signature and
/// be sealed under `current_epoch` (the newest the reader holds): a draft under
/// an older key is someone who still has that key, not the machine.
/// `key_for` gives the stream key of an epoch.
pub fn open_ephemeral(
    key_for: impl Fn(u32) -> Option<Zeroizing<[u8; 32]>>,
    stream: &StreamId,
    machine_key: &[u8; 32],
    current_epoch: u32,
    bytes: &[u8],
) -> Result<(u32, Vec<u8>), RecordError> {
    if bytes.len() < 4 + 12 + TAG_LEN + 64 {
        return Err(RecordError::Malformed("draft size"));
    }
    let (sealed, signature) = bytes.split_at(bytes.len() - 64);
    let signature: [u8; 64] = signature.try_into().expect("64 bytes");
    if !sig::verify(machine_key, sig::STREAM_DRAFT, &draft_signed(stream, sealed), &signature) {
        return Err(RecordError::BadHead);
    }
    let mut r = Reader::new(sealed);
    let epoch = r.u32()?;
    let nonce: [u8; 12] = r.array()?;
    let ct = r.rest();
    if ct.len() < TAG_LEN || ct.len() > MAX_CT {
        return Err(RecordError::Malformed("draft ciphertext size"));
    }
    if epoch != current_epoch {
        return Err(RecordError::Malformed("a draft under an old key"));
    }
    let key = key_for(epoch).ok_or(RecordError::NoKey(epoch))?;
    let pt = decrypt(&key, &nonce, &bound_aad(EPHEMERAL_AAD, stream, epoch), ct)?;
    let opened = unpack(&pt)?;
    if opened.fmt != FMT_DRAFT_V1 {
        return Err(RecordError::Malformed("not a draft"));
    }
    Ok((epoch, opened.body))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::record::tests::{machine, sid, MK};
    use crate::record::{stream_key, LogWriter};
    use crate::testing::rng;

    #[test]
    fn stream_names_are_fixed_per_machine_key_and_distinct() {
        assert_eq!(index_stream(&MK), index_stream(&MK));
        assert_ne!(index_stream(&MK), status_stream(&MK));
        assert_ne!(index_stream(&MK), index_stream(&[0x12; 32]));
        assert_ne!(index_stream(&MK), sid());
    }

    /// Relay-and-plans §5: a rotation's new key names every stream anew, and
    /// the old key (all a removed device holds) cannot compute those names.
    #[test]
    fn each_key_epoch_names_the_streams_anew_and_the_old_key_cannot_derive_the_new_names() {
        let (old, new) = (MK, [0x5b; 32]);
        for (source, id) in [(INDEX_SOURCE, ""), (STATUS_SOURCE, ""), ("claude", "s-1")] {
            let under_new = crate::record::stream_id(&new, source, id).unwrap();
            assert_ne!(crate::record::stream_id(&old, source, id).unwrap(), under_new);
        }
        assert!(reads_epoch_names(b"web/1 paths/1 streams/2"));
        assert!(!reads_epoch_names(b"web/1 paths/1"));
        assert!(!reads_epoch_names(b"streams/20"));
        assert!(!reads_epoch_names(&[0xff]));
    }

    #[test]
    fn pages_round_trip_are_named_by_their_hash_and_bound_to_stream_and_epoch() {
        let key = stream_key(&MK, &sid());
        let body = br#"{"fromIndex":0,"items":[{"kind":"user","text":"hi"}],"endIndex":1,"fp":"x"}"#;
        let page = seal_page(&key, &sid(), 0, FMT_FEED_V1, body, Compression::None, &mut rng(1)).unwrap();
        assert!(BUCKETS.contains(&(page.ct.len() - TAG_LEN)));
        let wire = page.encode();
        assert_eq!(SealedPage::decode(&wire).unwrap(), page);
        assert_eq!(page.hash(), page_hash(&wire));
        let opened = open_page(&key, &sid(), &page).unwrap();
        assert_eq!((opened.fmt, opened.body.as_slice()), (FMT_FEED_V1, &body[..]));
        // Another stream, another epoch, a flipped bit: all refused.
        let other = record::stream_id(&MK, "claude", "other").unwrap();
        assert_eq!(open_page(&key, &other, &page), Err(RecordError::Decrypt));
        assert_eq!(
            open_page(&key, &sid(), &SealedPage { epoch: 1, ..page.clone() }),
            Err(RecordError::Decrypt)
        );
        let mut flipped = page.clone();
        flipped.ct[3] ^= 1;
        assert_eq!(open_page(&key, &sid(), &flipped), Err(RecordError::Decrypt));
        // A page is not a record, nor a record a page, even under one key.
        let rec = record::seal(&key, &sid(), 0, 0, FMT_FEED_V1, &[0; 32], body, Compression::None, &mut rng(2)).unwrap();
        let as_page = SealedPage {
            epoch: 0,
            nonce: rec.nonce,
            ct: rec.ct,
        };
        assert_eq!(open_page(&key, &sid(), &as_page), Err(RecordError::Decrypt));
        // The largest body that fits, and one byte more.
        assert!(seal_page(&key, &sid(), 0, FMT_FEED_V1, &vec![b'a'; MAX_PAGE_BODY], Compression::None, &mut rng(3)).is_ok());
        assert!(seal_page(
            &key,
            &sid(),
            0,
            FMT_FEED_V1,
            &vec![b'a'; MAX_PAGE_BODY + 1],
            Compression::None,
            &mut rng(3)
        )
        .is_err());
        let big = seal_page(&key, &sid(), 0, FMT_FEED_V1, &vec![b'a'; MAX_PAGE_BODY], Compression::None, &mut rng(3)).unwrap();
        assert!(big.encode().len() <= MAX_PAGE_LEN);
    }

    fn checkpoint(next_seq: u64, link: [u8; 32], pages: Vec<[u8; 32]>) -> Manifest {
        let head = LogWriter::new(&MK, 0, sid(), next_seq, link).head(&machine(), 7_000);
        Manifest::sign(head, pages, &machine()).unwrap()
    }

    #[test]
    fn manifests_sign_both_the_head_and_the_page_list() {
        let m = checkpoint(5, [3; 32], vec![[1; 32], [2; 32]]);
        assert!(m.verify(&machine().verifying_key().to_bytes()));
        let wire = m.encode();
        let back = Manifest::decode(&wire).unwrap();
        assert_eq!(back, m);
        assert_eq!((back.upto_seq(), back.stream()), (5, sid()));
        let key = machine().verifying_key().to_bytes();
        // Reordered pages, a swapped page, another fmt: the signature no longer covers it.
        let mut swapped = m.clone();
        swapped.pages.swap(0, 1);
        assert!(!swapped.verify(&key));
        let mut other = m.clone();
        other.pages[1] = [9; 32];
        assert!(!other.verify(&key));
        // The head is checked too: a head signed by someone else, re-wrapped by the machine.
        let mut forged_head = m.head.clone();
        forged_head.signature[0] ^= 1;
        let rewrapped = Manifest::sign(forged_head, m.pages.clone(), &machine()).unwrap();
        assert!(!rewrapped.verify(&key));
        // Not the machine.
        assert!(!m.verify(&SigningKey::from_bytes(&[5; 32]).verifying_key().to_bytes()));
        let mut long = wire.clone();
        long.push(0);
        assert!(Manifest::decode(&long).is_err());
        assert!(Manifest::decode(&wire[..100]).is_err());
        assert!(Manifest::sign(m.head.clone(), vec![[0; 32]; MAX_PAGES + 1], &machine()).is_err());
    }

    #[test]
    fn a_reader_starts_from_a_manifest_and_follows_the_records_after_it() {
        let mut w = LogWriter::new(&MK, 0, sid(), 0, [0; 32]);
        for i in 0..3 {
            w.append(FMT_FEED_V1, format!("r{i}").as_bytes(), Compression::None, &mut rng(i)).unwrap();
        }
        let head = w.head(&machine(), 1_000);
        let manifest = Manifest::sign(head, vec![[4; 32]], &machine()).unwrap();
        let key = machine().verifying_key().to_bytes();
        let mut r = reader_from_manifest(key, &manifest).unwrap();
        r.add_epoch(0, &MK);
        assert_eq!(r.confirmed_next(), 3);
        let next = w.append(FMT_FEED_V1, b"r3", Compression::None, &mut rng(3)).unwrap();
        r.push(&next).unwrap();
        assert_eq!(r.confirm(&w.head(&machine(), 2_000)).unwrap()[0].body, b"r3");
        let mut bad = manifest.clone();
        bad.pages.push([0; 32]);
        assert!(reader_from_manifest(key, &bad).is_err());

        // A rebase: a fresh link at the same seq. A record extending the old
        // chain does not follow it.
        let rebased_head = LogWriter::new(&MK, 0, sid(), 4, [0xaa; 32]).head(&machine(), 3_000);
        let rebased = Manifest::sign(rebased_head, vec![], &machine()).unwrap();
        let mut r2 = reader_from_manifest(key, &rebased).unwrap();
        r2.add_epoch(0, &MK);
        let stale = w.append(FMT_FEED_V1, b"r4", Compression::None, &mut rng(4)).unwrap();
        assert_eq!(r2.push(&stale), Err(RecordError::BrokenChain(4)));
    }

    #[test]
    fn drafts_round_trip_signed_by_the_machine_bound_to_their_stream_and_current_epoch() {
        let key = stream_key(&MK, &sid());
        let m = machine();
        let mk_pub = m.verifying_key().to_bytes();
        let sealed = seal_ephemeral(&key, &sid(), 2, br#"{"turn":"t","v":3,"blocks":[]}"#, &m, &mut rng(1)).unwrap();
        let keys = |epoch: u32| (epoch == 2).then(|| stream_key(&MK, &sid()));
        let (epoch, body) = open_ephemeral(keys, &sid(), &mk_pub, 2, &sealed).unwrap();
        assert_eq!((epoch, body.as_slice()), (2, &br#"{"turn":"t","v":3,"blocks":[]}"#[..]));
        let other = record::stream_id(&MK, "claude", "other").unwrap();
        assert_eq!(
            open_ephemeral(|_| Some(stream_key(&MK, &sid())), &other, &mk_pub, 2, &sealed),
            Err(RecordError::BadHead),
            "another stream"
        );
        assert_eq!(open_ephemeral(|_| None, &sid(), &mk_pub, 2, &sealed), Err(RecordError::NoKey(2)));
        // Someone else holding the stream key (a removed device): not the machine's signature.
        let forger = ed25519_dalek::SigningKey::from_bytes(&[77; 32]);
        let forged = seal_ephemeral(&key, &sid(), 2, b"{}", &forger, &mut rng(3)).unwrap();
        assert_eq!(open_ephemeral(keys, &sid(), &mk_pub, 2, &forged), Err(RecordError::BadHead));
        // A draft under an epoch older than the newest the reader holds.
        assert!(open_ephemeral(|_| Some(stream_key(&MK, &sid())), &sid(), &mk_pub, 3, &sealed).is_err());
        // A page under the same key is not a draft.
        let page = seal_page(&key, &sid(), 2, FMT_DRAFT_V1, b"x", Compression::None, &mut rng(2)).unwrap();
        let mut as_draft = 2u32.to_be_bytes().to_vec();
        as_draft.extend_from_slice(&page.nonce);
        as_draft.extend_from_slice(&page.ct);
        let signature = sig::sign(&m, sig::STREAM_DRAFT, &[&sid().0[..], &as_draft].concat());
        as_draft.extend_from_slice(&signature);
        assert_eq!(open_ephemeral(keys, &sid(), &mk_pub, 2, &as_draft), Err(RecordError::Decrypt));
    }
}
