//! Encrypted log records: what the hub stores for offline reading.
//!
//! An agent is the single writer of each stream (a machine's session index, a
//! session's transcript, a machine's state). The hub stores what it is handed
//! and can say nothing about it beyond what [`SealedRecord`] shows:
//! `{seq, epoch, nonce, ct}`.
//!
//! - **Names.** `stream_id = HMAC-SHA256(K_id, source ‖ 0 ‖ id)` with
//!   `K_id = HKDF(salt = "rh/v1 stream-id", ikm = MK_e)`: the hub cannot map a
//!   stream to a CLI session id. Computed once, when the stream is created, and
//!   kept. `source` may not contain NUL, nor may `id`.
//! - **Keys.** `SK = HKDF-SHA256(salt = "rh/v1 stream-key", ikm = MK_e, info =
//!   stream_id)`. `MK` is only ever HKDF input, under a different salt for each
//!   role (stream ids, stream keys, and — later — the read-state document).
//! - **Binding.** The AAD is `"rh/v1 record\0" ‖ stream_id ‖ seq ‖ epoch`.
//! - **Linking.** Each record extends a running hash over the stream,
//!   `link_n = SHA-256("rh/v1 record-link\0" ‖ stream ‖ link_{n-1} ‖ record_n)`,
//!   and carries `link_{n-1}` in its plaintext.
//! - **Origin and freshness.** None of that stops a holder of the stream key —
//!   which includes a removed device for the epochs it held — from sealing a
//!   record of its own, nor the hub from hiding the newest records. So the
//!   agent signs **stream heads** with its machine key ([`StreamHead`]:
//!   `stream, next_seq, epoch, link, time`) after each append and as a periodic
//!   heartbeat, and a reader releases records only once a signed head covers
//!   them ([`LogReader::confirm`]); the newest head's time says how current the
//!   reader's view is. A head is also the checkpoint a reader can start from
//!   ([`LogReader::from_head`]) when older records have been compacted away.
//! - **Format.** The plaintext carries `fmt`.
//! - **Length.** A body may be raw-deflated (the writer's choice, per record:
//!   compress only bodies that do not mix attacker-influenced text with
//!   secrets, since compressed length leaks content) and is then padded to a
//!   size bucket, one record at a time.
//!
//! The plaintext layout is
//! `[flags u8][fmt u16][prev_link 32][body_len u32][body][zero padding]`.

use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use ed25519_dalek::SigningKey;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand_core::{CryptoRng, RngCore};
use sha2::{Digest, Sha256};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::codec::{check_u64, DecodeError, Reader};
use crate::sig;

/// A stream's opaque name.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StreamId(pub [u8; 32]);

impl std::fmt::Debug for StreamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StreamId(")?;
        for b in &self.0[..6] {
            write!(f, "{b:02x}")?;
        }
        write!(f, "…)")
    }
}

/// A record as the hub stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedRecord {
    pub seq: u64,
    pub epoch: u32,
    pub nonce: [u8; 12],
    pub ct: Vec<u8>,
}

/// A record decrypted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedRecord {
    pub seq: u64,
    pub fmt: u16,
    pub prev_link: [u8; 32],
    pub body: Vec<u8>,
}

/// Record format 1: today's feed window replacement, `{fromIndex, items, endIndex, fp}` as JSON
/// (a session stream; see [`crate::stream`] for every stream's content).
pub const FMT_FEED_V1: u16 = 1;
/// Record format 2: a machine's session index, `{reset?, upserts, removed}` as JSON.
pub const FMT_INDEX_V1: u16 = 2;
/// Record format 3: a machine's status (quota, accounts, versions, name) as JSON, whole each time.
pub const FMT_STATUS_V1: u16 = 3;
/// Format 4: a draft (never stored; only ever an `EPHEMERAL` payload).
pub const FMT_DRAFT_V1: u16 = 4;

/// Plaintext sizes are rounded up to one of these. The last is the largest
/// plaintext whose record still fits an `APPEND` frame with room to spare.
pub const BUCKETS: [usize; 9] = [256, 512, 1024, 2048, 4096, 8192, 16_384, 32_768, 61_440];
/// The largest padded plaintext.
pub const MAX_PLAINTEXT: usize = BUCKETS[BUCKETS.len() - 1];
/// The most a record's body may inflate to.
pub const MAX_BODY: usize = 4 * 1024 * 1024;
/// The largest ciphertext a record may carry.
pub const MAX_CT: usize = MAX_PLAINTEXT + TAG_LEN;
/// The longest `source` in a stream name, and the longest `id`.
pub const MAX_SOURCE_LEN: usize = 32;
pub const MAX_STREAM_ID_LEN: usize = 256;

pub(crate) const TAG_LEN: usize = 16;
const HEADER: usize = 1 + 2 + 32 + 4;
pub(crate) const FLAG_DEFLATE: u8 = 0x01;

const STREAM_ID_SALT: &[u8] = b"rh/v1 stream-id";
const STREAM_KEY_SALT: &[u8] = b"rh/v1 stream-key";
const AAD_LABEL: &[u8] = b"rh/v1 record\0";
const LINK_LABEL: &[u8] = b"rh/v1 record-link\0";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RecordError {
    #[error("the record does not decrypt under this stream's key (tampered, moved, or the wrong epoch)")]
    Decrypt,
    #[error("no key for epoch {0}")]
    NoKey(u32),
    #[error("malformed record plaintext: {0}")]
    Malformed(&'static str),
    #[error("the body does not fit a record ({0} bytes after compression)")]
    TooLarge(usize),
    #[error("expected seq {expected}, got {got}: a record was replayed, reordered or dropped")]
    OutOfOrder { expected: u64, got: u64 },
    #[error("the link is broken at seq {0}: a record before it was removed or replaced")]
    BrokenChain(u64),
    #[error("epoch went backwards at seq {seq}: {from} -> {to}")]
    EpochRollback { seq: u64, from: u32, to: u32 },
    #[error("a stream name's source must be 1-{MAX_SOURCE_LEN} bytes and neither part may contain NUL")]
    BadName,
    #[error("the stream head's signature is not the machine's")]
    BadHead,
    #[error("the signed head does not match the records received: they were forged or replaced")]
    Forged,
    #[error("the machine signed records up to {head} but only {have} arrived: some are being withheld")]
    Incomplete { have: u64, head: u64 },
    #[error(transparent)]
    Decode(#[from] DecodeError),
}

/// Whether a writer may compress a record's body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// The default: no length-based leak, whatever the body mixes.
    None,
    /// Raw deflate when it helps. Only for bodies that do not mix
    /// attacker-influenced text with secrets.
    Deflate,
}

fn id_key(mk: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let mut out = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(STREAM_ID_SALT), mk).expand(&[], out.as_mut()).expect("32 bytes");
    out
}

/// The opaque name of the stream for `source`/`id` under `mk`.
pub fn stream_id(mk: &[u8; 32], source: &str, id: &str) -> Result<StreamId, RecordError> {
    if source.is_empty() || source.len() > MAX_SOURCE_LEN || id.len() > MAX_STREAM_ID_LEN || source.contains('\0') || id.contains('\0') {
        return Err(RecordError::BadName);
    }
    let key = id_key(mk);
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key.as_ref()).expect("HMAC takes any key length");
    mac.update(source.as_bytes());
    mac.update(&[0]);
    mac.update(id.as_bytes());
    Ok(StreamId(mac.finalize().into_bytes().into()))
}

/// The key a stream's records are sealed with under `mk` (one epoch's machine key).
pub fn stream_key(mk: &[u8; 32], stream: &StreamId) -> Zeroizing<[u8; 32]> {
    let mut out = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(STREAM_KEY_SALT), mk)
        .expand(&stream.0, out.as_mut())
        .expect("32 bytes is a valid HKDF-SHA256 length");
    out
}

/// The bucket a plaintext of `len` bytes is padded to.
pub fn bucket(len: usize) -> Option<usize> {
    BUCKETS.iter().copied().find(|&b| b >= len)
}

fn aad(stream: &StreamId, seq: u64, epoch: u32) -> Vec<u8> {
    let mut aad = Vec::with_capacity(AAD_LABEL.len() + 32 + 12);
    aad.extend_from_slice(AAD_LABEL);
    aad.extend_from_slice(&stream.0);
    aad.extend_from_slice(&seq.to_be_bytes());
    aad.extend_from_slice(&epoch.to_be_bytes());
    aad
}

impl SealedRecord {
    /// `seq u64 | epoch u32 | nonce 12 | ct_len u32 | ct`.
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&self.epoch.to_be_bytes());
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&(self.ct.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.ct);
    }

    pub(crate) fn decode_from(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let seq = r.u64()?;
        let epoch = r.u32()?;
        let nonce = r.array()?;
        let len = r.u32()? as usize;
        if len > MAX_CT {
            return Err(DecodeError::TooLarge {
                what: "record ciphertext",
                len,
                max: MAX_CT,
            });
        }
        if len < TAG_LEN {
            return Err(DecodeError::Invalid("record ciphertext shorter than its tag"));
        }
        Ok(Self {
            seq,
            epoch,
            nonce,
            ct: r.bytes(len)?.to_vec(),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let record = Self::decode_from(&mut r)?;
        r.finish()?;
        Ok(record)
    }

    /// The stream's running hash after this record, given the one before it.
    pub fn link(&self, stream: &StreamId, prev_link: &[u8; 32]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(LINK_LABEL);
        h.update(stream.0);
        h.update(prev_link);
        h.update(self.encode());
        h.finalize().into()
    }
}

/// Seal one record. The nonce comes from `rng`: a writer that loses a CAS and
/// re-seals the same seq must not reuse a nonce, which a seq-derived one would.
#[allow(clippy::too_many_arguments)]
pub fn seal<R: RngCore + CryptoRng>(
    key: &[u8; 32],
    stream: &StreamId,
    seq: u64,
    epoch: u32,
    fmt: u16,
    prev_link: &[u8; 32],
    body: &[u8],
    compression: Compression,
    rng: &mut R,
) -> Result<SealedRecord, RecordError> {
    let mut nonce = [0u8; 12];
    rng.fill_bytes(&mut nonce);
    seal_with_nonce(key, stream, seq, epoch, fmt, prev_link, body, compression, nonce)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn seal_with_nonce(
    key: &[u8; 32],
    stream: &StreamId,
    seq: u64,
    epoch: u32,
    fmt: u16,
    prev_link: &[u8; 32],
    body: &[u8],
    compression: Compression,
    nonce: [u8; 12],
) -> Result<SealedRecord, RecordError> {
    check_u64(seq)?;
    if body.len() > MAX_BODY {
        return Err(RecordError::TooLarge(body.len()));
    }
    let compressed = match compression {
        Compression::Deflate => deflate(body),
        Compression::None => None,
    };
    let (flags, stored) = match compressed {
        Some(ref c) if c.len() < body.len() => (FLAG_DEFLATE, c.as_slice()),
        _ => (0, body),
    };
    let padded = bucket(HEADER + stored.len()).ok_or(RecordError::TooLarge(stored.len()))?;
    let mut pt = Zeroizing::new(Vec::with_capacity(padded));
    pt.push(flags);
    pt.extend_from_slice(&fmt.to_be_bytes());
    pt.extend_from_slice(prev_link);
    pt.extend_from_slice(&(stored.len() as u32).to_be_bytes());
    pt.extend_from_slice(stored);
    pt.resize(padded, 0);
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    let ct = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &pt,
                aad: &aad(stream, seq, epoch),
            },
        )
        .map_err(|_| RecordError::Decrypt)?;
    Ok(SealedRecord { seq, epoch, nonce, ct })
}

/// Decrypt one record and check its plaintext is well formed. Says nothing
/// about where the record sits in its stream or who wrote it — that is
/// [`LogReader`]'s job.
pub fn open(key: &[u8; 32], stream: &StreamId, record: &SealedRecord) -> Result<OpenedRecord, RecordError> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    let pt = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&record.nonce),
                Payload {
                    msg: &record.ct,
                    aad: &aad(stream, record.seq, record.epoch),
                },
            )
            .map_err(|_| RecordError::Decrypt)?,
    );
    if !BUCKETS.contains(&pt.len()) {
        return Err(RecordError::Malformed("plaintext is not a bucket size"));
    }
    let mut r = Reader::new(&pt);
    let flags = r.u8()?;
    if flags & !FLAG_DEFLATE != 0 {
        return Err(RecordError::Malformed("unknown flags"));
    }
    let fmt = r.u16()?;
    let prev_link = r.array()?;
    let len = r.u32()? as usize;
    if len > r.remaining() {
        return Err(RecordError::Malformed("body longer than the record"));
    }
    let stored = r.bytes(len)?;
    if r.rest().iter().any(|&b| b != 0) {
        return Err(RecordError::Malformed("non-zero padding"));
    }
    let body = if flags & FLAG_DEFLATE != 0 { inflate(stored)? } else { stored.to_vec() };
    Ok(OpenedRecord {
        seq: record.seq,
        fmt,
        prev_link,
        body,
    })
}

pub(crate) fn deflate(body: &[u8]) -> Option<Vec<u8>> {
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(body).ok()?;
    encoder.finish().ok()
}

pub(crate) fn inflate(stored: &[u8]) -> Result<Vec<u8>, RecordError> {
    let mut out = Vec::new();
    let mut decoder = flate2::read::DeflateDecoder::new(stored).take(MAX_BODY as u64 + 1);
    decoder
        .read_to_end(&mut out)
        .map_err(|_| RecordError::Malformed("body does not inflate"))?;
    if out.len() > MAX_BODY {
        return Err(RecordError::Malformed("body inflates past the limit"));
    }
    Ok(out)
}

/// The machine's signed statement: stream `stream` holds records `0..next_seq`,
/// the last of them in `epoch`, with running hash `link`, as of `time_ms`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamHead {
    pub stream: StreamId,
    pub next_seq: u64,
    pub epoch: u32,
    pub link: [u8; 32],
    pub time_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedHead {
    pub head: StreamHead,
    pub signature: [u8; 64],
}

impl StreamHead {
    /// `stream | u64 next_seq | u32 epoch | link | u64 time`.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + 8 + 4 + 32 + 8);
        out.extend_from_slice(&self.stream.0);
        out.extend_from_slice(&self.next_seq.to_be_bytes());
        out.extend_from_slice(&self.epoch.to_be_bytes());
        out.extend_from_slice(&self.link);
        out.extend_from_slice(&self.time_ms.to_be_bytes());
        out
    }

    pub fn sign(self, machine: &SigningKey) -> SignedHead {
        let signature = sig::sign(machine, sig::STREAM_HEAD, &self.encode());
        SignedHead { head: self, signature }
    }
}

impl SignedHead {
    pub fn encode(&self) -> Vec<u8> {
        [self.head.encode(), self.signature.to_vec()].concat()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let head = StreamHead {
            stream: StreamId(r.array()?),
            next_seq: r.u64()?,
            epoch: r.u32()?,
            link: r.array()?,
            time_ms: r.u64()?,
        };
        let signature = r.array()?;
        r.finish()?;
        Ok(Self { head, signature })
    }

    pub fn verify(&self, machine_key: &[u8; 32]) -> bool {
        sig::verify(machine_key, sig::STREAM_HEAD, &self.head.encode(), &self.signature)
    }
}

/// The writer's side of one stream: the next seq and the link it must extend.
#[derive(Clone)]
pub struct LogWriter {
    stream: StreamId,
    key: Zeroizing<[u8; 32]>,
    epoch: u32,
    next_seq: u64,
    link: [u8; 32],
}

impl LogWriter {
    /// A new stream, or one resumed at `next_seq` after the record whose link is `link`.
    pub fn new(mk: &[u8; 32], epoch: u32, stream: StreamId, next_seq: u64, link: [u8; 32]) -> Self {
        Self {
            key: stream_key(mk, &stream),
            stream,
            epoch,
            next_seq,
            link,
        }
    }

    pub fn append<R: RngCore + CryptoRng>(
        &mut self,
        fmt: u16,
        body: &[u8],
        compression: Compression,
        rng: &mut R,
    ) -> Result<SealedRecord, RecordError> {
        let record = seal(
            &self.key,
            &self.stream,
            self.next_seq,
            self.epoch,
            fmt,
            &self.link,
            body,
            compression,
            rng,
        )?;
        self.link = record.link(&self.stream, &self.link);
        self.next_seq += 1;
        Ok(record)
    }

    /// The head covering everything appended so far, signed with the machine
    /// key: after each append batch, and periodically as a heartbeat.
    pub fn head(&self, machine: &SigningKey, time_ms: u64) -> SignedHead {
        StreamHead {
            stream: self.stream,
            next_seq: self.next_seq,
            epoch: self.epoch,
            link: self.link,
            time_ms,
        }
        .sign(machine)
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// The running hash after the last record appended.
    pub fn link(&self) -> [u8; 32] {
        self.link
    }

    pub fn stream(&self) -> StreamId {
        self.stream
    }

    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    /// The stream key this writer seals with (pages and drafts use it too).
    pub fn key(&self) -> &[u8; 32] {
        &self.key
    }
}

/// The reader's side: every record, in order, linked to the last — and none
/// released until a head signed by the machine covers it.
pub struct LogReader {
    stream: StreamId,
    machine_key: [u8; 32],
    keys: BTreeMap<u32, Zeroizing<[u8; 32]>>,
    /// Released up to here, with the link and epoch a head must show there.
    confirmed_next: u64,
    confirmed_link: [u8; 32],
    confirmed_epoch: u32,
    /// Received (decrypted, linked) up to here.
    next_seq: u64,
    link: [u8; 32],
    epoch: u32,
    pending: VecDeque<(OpenedRecord, [u8; 32], u32)>,
    freshest_ms: Option<u64>,
}

impl LogReader {
    /// From the start of a stream written by the machine whose Ed25519 key is `machine_key`.
    pub fn new(stream: StreamId, machine_key: [u8; 32]) -> Self {
        Self {
            stream,
            machine_key,
            keys: BTreeMap::new(),
            confirmed_next: 0,
            confirmed_link: [0; 32],
            confirmed_epoch: 0,
            next_seq: 0,
            link: [0; 32],
            epoch: 0,
            pending: VecDeque::new(),
            freshest_ms: None,
        }
    }

    /// From a signed head (a checkpoint after which older records were compacted).
    pub fn from_head(machine_key: [u8; 32], head: &SignedHead) -> Result<Self, RecordError> {
        if !head.verify(&machine_key) {
            return Err(RecordError::BadHead);
        }
        let h = &head.head;
        Ok(Self {
            stream: h.stream,
            machine_key,
            keys: BTreeMap::new(),
            confirmed_next: h.next_seq,
            confirmed_link: h.link,
            confirmed_epoch: h.epoch,
            next_seq: h.next_seq,
            link: h.link,
            epoch: h.epoch,
            pending: VecDeque::new(),
            freshest_ms: Some(h.time_ms),
        })
    }

    /// From a head this reader released before and kept, checked under
    /// `signer` — a key the machine held when it signed that head, which may
    /// be one it renewed away from since (`docs/host-identity-rotation.md`
    /// §5.4) — and from then on checking every head under `machine_key`, the
    /// machine's key now.
    pub fn from_stored_head(machine_key: [u8; 32], signer: &[u8; 32], head: &SignedHead) -> Result<Self, RecordError> {
        let mut reader = Self::from_head(*signer, head)?;
        reader.machine_key = machine_key;
        Ok(reader)
    }

    /// Learn `MK` for one epoch.
    pub fn add_epoch(&mut self, epoch: u32, mk: &[u8; 32]) {
        self.keys.insert(epoch, stream_key(mk, &self.stream));
    }

    /// Take the next record. It is checked and held, not yet released.
    pub fn push(&mut self, record: &SealedRecord) -> Result<(), RecordError> {
        if record.seq != self.next_seq {
            return Err(RecordError::OutOfOrder {
                expected: self.next_seq,
                got: record.seq,
            });
        }
        if record.epoch < self.epoch {
            return Err(RecordError::EpochRollback {
                seq: record.seq,
                from: self.epoch,
                to: record.epoch,
            });
        }
        let key = self.keys.get(&record.epoch).ok_or(RecordError::NoKey(record.epoch))?;
        let opened = open(key, &self.stream, record)?;
        if opened.prev_link != self.link {
            return Err(RecordError::BrokenChain(record.seq));
        }
        self.link = record.link(&self.stream, &self.link);
        self.next_seq += 1;
        self.epoch = record.epoch;
        self.pending.push_back((opened, self.link, record.epoch));
        Ok(())
    }

    /// Release the records a signed head covers. A head older than what is
    /// already confirmed refreshes nothing; a head beyond what arrived means
    /// records are being withheld; a head whose link differs from the records
    /// received means they were forged — and a head at exactly what is already
    /// confirmed (a heartbeat) must still show the confirmed link.
    ///
    /// On [`RecordError::Forged`] or [`RecordError::Incomplete`] every record
    /// not yet released is dropped and the reader is back at
    /// [`LogReader::confirmed_next`], so its owner can fetch from there again
    /// instead of being stuck behind records it can never release.
    pub fn confirm(&mut self, head: &SignedHead) -> Result<Vec<OpenedRecord>, RecordError> {
        if head.head.stream != self.stream || !head.verify(&self.machine_key) {
            return Err(RecordError::BadHead);
        }
        let h = &head.head;
        if h.next_seq < self.confirmed_next {
            return Ok(Vec::new());
        }
        if h.next_seq > self.next_seq {
            let error = RecordError::Incomplete {
                have: self.next_seq,
                head: h.next_seq,
            };
            self.roll_back();
            return Err(error);
        }
        let covered = (h.next_seq - self.confirmed_next) as usize;
        let (link, epoch) = if covered > 0 {
            let (_, link, epoch) = &self.pending[covered - 1];
            (*link, *epoch)
        } else {
            (self.confirmed_link, self.confirmed_epoch)
        };
        // Nothing was ever written: no record carries an epoch to compare.
        let epoch_matters = h.next_seq > 0;
        if link != h.link || (epoch_matters && epoch != h.epoch) {
            self.roll_back();
            return Err(RecordError::Forged);
        }
        self.freshest_ms = Some(self.freshest_ms.map_or(h.time_ms, |t| t.max(h.time_ms)));
        self.confirmed_next = h.next_seq;
        self.confirmed_link = link;
        self.confirmed_epoch = if epoch_matters { epoch } else { self.confirmed_epoch };
        Ok(self.pending.drain(..covered).map(|(opened, ..)| opened).collect())
    }

    /// Forget everything received but not released.
    fn roll_back(&mut self) {
        self.pending.clear();
        self.next_seq = self.confirmed_next;
        self.link = self.confirmed_link;
        self.epoch = self.confirmed_epoch;
    }

    /// The time of the newest head the machine signed for this stream: how
    /// current the reader's view is known to be.
    pub fn freshest_ms(&self) -> Option<u64> {
        self.freshest_ms
    }

    pub fn confirmed_next(&self) -> u64 {
        self.confirmed_next
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testing::rng;

    pub(crate) const MK: [u8; 32] = [0x11; 32];

    pub(crate) fn machine() -> SigningKey {
        SigningKey::from_bytes(&[0x42; 32])
    }

    pub(crate) fn sid() -> StreamId {
        stream_id(&MK, "claude", "8f0c2d4e-0000-4000-8000-000000000001").unwrap()
    }

    fn reader() -> LogReader {
        let mut r = LogReader::new(sid(), machine().verifying_key().to_bytes());
        r.add_epoch(0, &MK);
        r
    }

    #[test]
    fn records_round_trip_pad_to_a_bucket_and_release_only_under_a_signed_head() {
        let mut w = LogWriter::new(&MK, 0, sid(), 0, [0; 32]);
        let mut r = reader();
        let bodies: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"hello".to_vec(),
            vec![b'a'; 10_000],
            (0..=255u8).cycle().take(5000).collect(),
        ];
        for (i, body) in bodies.iter().enumerate() {
            let compression = if i == 2 { Compression::Deflate } else { Compression::None };
            let sealed = w.append(FMT_FEED_V1, body, compression, &mut rng(1)).unwrap();
            assert!(BUCKETS.contains(&(sealed.ct.len() - TAG_LEN)), "padded to a bucket");
            r.push(&sealed).unwrap();
        }
        assert_eq!(r.confirmed_next(), 0, "nothing released before a head");
        let released = r.confirm(&w.head(&machine(), 5_000)).unwrap();
        assert_eq!(released.iter().map(|o| o.body.clone()).collect::<Vec<_>>(), bodies);
        assert_eq!(r.freshest_ms(), Some(5_000));
        // A heartbeat: same records, newer time.
        assert!(r.confirm(&w.head(&machine(), 9_000)).unwrap().is_empty());
        assert_eq!(r.freshest_ms(), Some(9_000));
    }

    #[test]
    fn length_leaks_only_the_bucket() {
        let a = seal(&[1; 32], &sid(), 0, 0, 1, &[0; 32], &[7u8; 30], Compression::None, &mut rng(1)).unwrap();
        let b = seal(
            &[1; 32],
            &sid(),
            0,
            0,
            1,
            &[0; 32],
            b"a different 30 byte message!!!",
            Compression::None,
            &mut rng(2),
        )
        .unwrap();
        assert_eq!(a.ct.len(), b.ct.len());
    }

    #[test]
    fn a_record_cannot_be_moved_to_another_stream_seq_or_epoch() {
        let key = stream_key(&MK, &sid());
        let sealed = seal(&key, &sid(), 3, 1, 1, &[0; 32], b"secret", Compression::None, &mut rng(3)).unwrap();
        assert!(open(&key, &sid(), &sealed).is_ok());
        let other = stream_id(&MK, "claude", "another").unwrap();
        assert_eq!(open(&key, &other, &sealed), Err(RecordError::Decrypt));
        assert_eq!(open(&key, &sid(), &SealedRecord { seq: 4, ..sealed.clone() }), Err(RecordError::Decrypt));
        assert_eq!(
            open(&key, &sid(), &SealedRecord { epoch: 2, ..sealed.clone() }),
            Err(RecordError::Decrypt)
        );
        let mut flipped = sealed.clone();
        flipped.ct[0] ^= 1;
        assert_eq!(open(&key, &sid(), &flipped), Err(RecordError::Decrypt));
    }

    #[test]
    fn stream_names_refuse_nul_and_empty_sources() {
        assert_eq!(stream_id(&MK, "", "x"), Err(RecordError::BadName));
        assert_eq!(stream_id(&MK, "cla\0ude", "x"), Err(RecordError::BadName));
        assert_eq!(stream_id(&MK, "claude", "x\0y"), Err(RecordError::BadName));
        assert_ne!(stream_id(&MK, "claude", "ab").unwrap(), stream_id(&MK, "claudea", "b").unwrap());
    }

    #[test]
    fn the_reader_rejects_replay_reorder_deletion_and_epoch_rollback() {
        let mut w = LogWriter::new(&MK, 0, sid(), 0, [0; 32]);
        let recs: Vec<_> = (0..4)
            .map(|i| w.append(1, format!("r{i}").as_bytes(), Compression::None, &mut rng(i)).unwrap())
            .collect();
        let mut r = reader();
        r.push(&recs[0]).unwrap();
        assert_eq!(r.push(&recs[0]), Err(RecordError::OutOfOrder { expected: 1, got: 0 }), "replay");
        assert_eq!(r.push(&recs[2]), Err(RecordError::OutOfOrder { expected: 1, got: 2 }), "skip");
        let mut w2 = LogWriter::new(&MK, 0, sid(), 1, [9; 32]);
        assert_eq!(
            r.push(&w2.append(1, b"forged", Compression::None, &mut rng(9)).unwrap()),
            Err(RecordError::BrokenChain(1))
        );

        let mk2 = [0x22; 32];
        let mut r = reader();
        r.add_epoch(1, &mk2);
        let mut w1 = LogWriter::new(&mk2, 1, sid(), 0, [0; 32]);
        r.push(&w1.append(1, b"new epoch", Compression::None, &mut rng(5)).unwrap()).unwrap();
        let mut w0 = LogWriter::new(&MK, 0, sid(), 1, [0; 32]);
        assert!(matches!(
            r.push(&w0.append(1, b"old", Compression::None, &mut rng(6)).unwrap()),
            Err(RecordError::EpochRollback { .. })
        ));
    }

    #[test]
    fn heads_catch_forgery_withholding_and_bad_signatures_and_serve_as_checkpoints() {
        let mut w = LogWriter::new(&MK, 0, sid(), 0, [0; 32]);
        let recs: Vec<_> = (0..3)
            .map(|i| w.append(1, format!("r{i}").as_bytes(), Compression::None, &mut rng(i)).unwrap())
            .collect();
        let head = w.head(&machine(), 1_000);
        // Withheld: the head covers three records, two arrived.
        let mut r = reader();
        r.push(&recs[0]).unwrap();
        r.push(&recs[1]).unwrap();
        assert_eq!(r.confirm(&head), Err(RecordError::Incomplete { have: 2, head: 3 }));
        // A head not signed by the machine.
        let fake = head.head.clone().sign(&SigningKey::from_bytes(&[7; 32]));
        assert_eq!(r.confirm(&fake), Err(RecordError::BadHead));
        // From a head on: a later reader starts after the compacted records.
        let mut late = LogReader::from_head(machine().verifying_key().to_bytes(), &head).unwrap();
        late.add_epoch(0, &MK);
        let next = w.append(1, b"r3", Compression::None, &mut rng(3)).unwrap();
        late.push(&next).unwrap();
        assert_eq!(late.confirm(&w.head(&machine(), 2_000)).unwrap()[0].body, b"r3");
        assert!(LogReader::from_head([1; 32], &head).is_err());
    }

    #[test]
    fn a_reader_recovers_from_a_forged_or_withheld_batch_and_checks_heartbeats() {
        let mut w = LogWriter::new(&MK, 0, sid(), 0, [0; 32]);
        let good: Vec<_> = (0..2)
            .map(|i| w.append(1, format!("r{i}").as_bytes(), Compression::None, &mut rng(i)).unwrap())
            .collect();
        let head = w.head(&machine(), 1_000);
        // A thief holding the stream key seals its own record 0 and the hub serves it.
        let mut thief = LogWriter::new(&MK, 0, sid(), 0, [0; 32]);
        let forged = thief.append(1, b"forged", Compression::None, &mut rng(9)).unwrap();
        let mut r = reader();
        r.push(&forged).unwrap();
        assert_eq!(r.confirm(&w.head(&machine(), 900)), Err(RecordError::Incomplete { have: 1, head: 2 }));
        // Rolled back: the real records can be fetched from 0 again.
        r.push(&forged).unwrap();
        r.push(&good[1]).unwrap_err();
        let mut r = reader();
        r.push(&forged).unwrap();
        let one = {
            let mut w1 = LogWriter::new(&MK, 0, sid(), 0, [0; 32]);
            w1.append(1, b"r0", Compression::None, &mut rng(0)).unwrap();
            w1.head(&machine(), 950)
        };
        assert_eq!(r.confirm(&one), Err(RecordError::Forged));
        assert_eq!(r.confirmed_next(), 0);
        for record in &good {
            r.push(record).unwrap();
        }
        assert_eq!(r.confirm(&head).unwrap().len(), 2, "not stuck behind the forgery");

        // A heartbeat at the confirmed seq is still checked against the link.
        let mut lying = head.head.clone();
        lying.link = [7; 32];
        lying.time_ms = 2_000;
        assert_eq!(r.confirm(&lying.sign(&machine())), Err(RecordError::Forged));
        assert_eq!(r.freshest_ms(), Some(1_000), "a lying heartbeat refreshes nothing");
        assert!(r.confirm(&w.head(&machine(), 3_000)).unwrap().is_empty());
        assert_eq!(r.freshest_ms(), Some(3_000));
        // And an empty stream's heartbeat must show the empty link.
        let mut fresh = reader();
        let empty = LogWriter::new(&MK, 0, sid(), 0, [0; 32]).head(&machine(), 10);
        assert!(fresh.confirm(&empty).unwrap().is_empty());
        let mut bogus = empty.head.clone();
        bogus.link = [1; 32];
        assert_eq!(fresh.confirm(&bogus.sign(&machine())), Err(RecordError::Forged));
    }

    #[test]
    fn malformed_plaintext_is_refused_even_under_the_right_key() {
        let key = stream_key(&MK, &sid());
        let cipher = Aes256Gcm::new_from_slice(key.as_ref()).unwrap();
        let seal_raw = |pt: &[u8]| SealedRecord {
            seq: 0,
            epoch: 0,
            nonce: [0; 12],
            ct: cipher
                .encrypt(
                    Nonce::from_slice(&[0; 12]),
                    Payload {
                        msg: pt,
                        aad: &aad(&sid(), 0, 0),
                    },
                )
                .unwrap(),
        };
        let mut good = vec![0u8; 256];
        good[38] = 1; // body_len = 1
        assert!(open(&key, &sid(), &seal_raw(&good)).is_ok());
        let mut bad_pad = good.clone();
        bad_pad[255] = 1;
        assert!(matches!(open(&key, &sid(), &seal_raw(&bad_pad)), Err(RecordError::Malformed(_))));
        assert!(matches!(open(&key, &sid(), &seal_raw(&good[..200])), Err(RecordError::Malformed(_))));
        let mut bad_flags = good.clone();
        bad_flags[0] = 0x80;
        assert!(matches!(open(&key, &sid(), &seal_raw(&bad_flags)), Err(RecordError::Malformed(_))));
    }

    #[test]
    fn oversized_bodies_are_refused_and_compression_is_opt_in() {
        let mut noise = vec![0u8; 70_000];
        rand_core::RngCore::fill_bytes(&mut rng(7), &mut noise);
        assert!(matches!(
            seal(&[1; 32], &sid(), 0, 0, 1, &[0; 32], &noise, Compression::Deflate, &mut rng(1)),
            Err(RecordError::TooLarge(_))
        ));
        let text = b"line of tool output\n".repeat(400);
        let plain = seal(&[1; 32], &sid(), 0, 0, 1, &[0; 32], &text, Compression::None, &mut rng(1)).unwrap();
        let packed = seal(&[1; 32], &sid(), 0, 0, 1, &[0; 32], &text, Compression::Deflate, &mut rng(1)).unwrap();
        assert!(packed.ct.len() < plain.ct.len());
    }

    #[test]
    fn sealed_records_and_heads_round_trip_their_wire_form() {
        let sealed = seal(&[1; 32], &sid(), 9, 2, 1, &[0; 32], b"x", Compression::None, &mut rng(1)).unwrap();
        assert_eq!(SealedRecord::decode(&sealed.encode()).unwrap(), sealed);
        let mut bytes = sealed.encode();
        bytes.push(0);
        assert!(SealedRecord::decode(&bytes).is_err());
        let head = StreamHead {
            stream: sid(),
            next_seq: 3,
            epoch: 1,
            link: [5; 32],
            time_ms: 9,
        }
        .sign(&machine());
        assert_eq!(SignedHead::decode(&head.encode()).unwrap(), head);
    }
}
