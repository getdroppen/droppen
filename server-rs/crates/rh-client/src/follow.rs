//! Following one stream for good, as the local install keeps its copies
//! (`docs/local-aggregator.md`, S1-5): the reader of [`crate::streams`], but
//! resumable and sans-IO.
//!
//! A [`Follower`] takes the frames of one subscription (`SUB {ch, stream,
//! from_seq}` on a [`crate::link::Link`]) and says what they mean
//! ([`Step`]). It releases nothing a machine-signed head does not cover, and
//! its cursor ([`Follower::head`]) is the last such head: kept with what was
//! folded from the bodies, it lets a later follower carry on where this one
//! stopped ([`Follower::resume`], subscribing from [`Follower::from_seq`])
//! instead of reading the stream again.
//!
//! - `GONE` (the hub compacted the tail under the reader): everything starts
//!   again; the next bodies released replace what was folded (`reset`).
//! - `CHECKPOINT`: the caller fetches the manifest's pages and hands them to
//!   [`Follower::checkpoint`], which checks each against the signed list.
//! - A record that does not open or link, a head the records do not match,
//!   a refused subscription: an error. The caller drops the follower (and
//!   keeps what it folded so far) and reads the stream again from the start,
//!   later.
//!
//! **A hostile hub** can withhold, but it cannot forge or roll back:
//!
//! - a checkpoint's manifest is checked against the machine's signature the
//!   moment it arrives, before a single page is fetched, and each page
//!   against the manifest as it comes ([`check_page`]);
//! - a [`Floor`] — the newest head the copy was seen at, kept with it and
//!   handed to the follower that reads the stream again — is checked where
//!   results become visible: at the end of the catch-up (`stream_synced`)
//!   and on every head after it. A head there that is older in time and not
//!   past the floor in sequence means the hub replays the machine's own
//!   older, genuine signatures, or withholds what came after: refused, and
//!   the copy the caller holds stays. During the catch-up anything genuine
//!   is taken — an honest hub's newest checkpoint is usually older than its
//!   newest records. `GONE` is taken only for records past where the reader
//!   stands. A stream the machine deleted and wrote again from 0 has newer
//!   heads, so it is still read.

use rh_proto::frame::Frame;
use rh_proto::record::{LogReader, SignedHead, StreamHead, StreamId};
use rh_proto::stream::{open_page, page_hash, reader_from_manifest, Manifest, SealedPage};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::link::CONTROL_FRAME;

/// What a frame meant.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing for the caller.
    Nothing,
    /// The hub has no such stream (yet): the subscription stays and follows.
    Missing,
    /// The catch-up ended: what was released so far is the stream as the
    /// hub holds it.
    Synced,
    /// Start from this manifest: fetch its pages (in order), then
    /// [`Follower::checkpoint`].
    Checkpoint(Box<Manifest>),
    /// Bodies a machine-signed head released, in order, with their format;
    /// `reset`: they replace everything released before (a checkpoint's
    /// pages, or the stream read again after `GONE`). Empty when a head only
    /// said the stream is still current.
    Released { reset: bool, bodies: Vec<(u16, Vec<u8>)> },
}

/// Why following stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowError {
    /// The hub refused the subscription (`stream_refused`): its code.
    Refused(String),
    /// What arrived does not verify, open or link: read the stream again.
    Broken(String),
}

impl std::fmt::Display for FollowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FollowError::Refused(code) => write!(f, "the hub refused the subscription: {code}"),
            FollowError::Broken(why) => f.write_str(why),
        }
    }
}

fn broken(e: impl std::fmt::Display) -> FollowError {
    FollowError::Broken(e.to_string())
}

/// The newest a copy is known to have been: its last released head's place
/// in the stream and the machine's time on it. Whatever is older on both
/// counts is a rollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Floor {
    pub next_seq: u64,
    pub time_ms: u64,
}

/// A head signed further ahead of this device's clock than this does not
/// raise a floor's time past that clock.
pub const AHEAD_MAX_MS: u64 = 24 * 60 * 60 * 1000;

impl Floor {
    /// The floor a released head sets, by this device's clock now.
    pub fn of(head: &SignedHead) -> Self {
        Self::at(head, crate::now_ms())
    }

    /// The floor a released head sets at `now_ms`. A machine whose clock ran
    /// more than [`AHEAD_MAX_MS`] ahead (say, by a year) would otherwise set
    /// a floor no honest head reaches until then — a stream it rewrites from
    /// 0 after its clock is put right would be refused for that long. Such a
    /// head's time counts as `now_ms`.
    pub fn at(head: &SignedHead, now_ms: u64) -> Self {
        let time_ms = head.head.time_ms;
        Self {
            next_seq: head.head.next_seq,
            time_ms: if time_ms > now_ms.saturating_add(AHEAD_MAX_MS) {
                now_ms
            } else {
                time_ms
            },
        }
    }

    /// Whether `head` would move the copy back: signed earlier and not past
    /// this floor in the stream.
    pub fn undercut_by(&self, head: &StreamHead) -> bool {
        head.time_ms < self.time_ms && head.next_seq <= self.next_seq
    }

    /// The newer of the two on each count.
    fn raise(self, other: Floor) -> Floor {
        Floor {
            next_seq: self.next_seq.max(other.next_seq),
            time_ms: self.time_ms.max(other.time_ms),
        }
    }
}

/// Check page `i` of `manifest` as it arrives, before the next is fetched.
pub fn check_page(manifest: &Manifest, i: usize, data: &[u8]) -> Result<(), FollowError> {
    match manifest.pages.get(i) {
        Some(hash) if page_hash(data) == *hash => Ok(()),
        _ => Err(broken("a page that is not the one the manifest names")),
    }
}

/// One stream, followed.
pub struct Follower {
    stream: StreamId,
    machine_key: [u8; 32],
    keys: Vec<(u32, Zeroizing<[u8; 32]>)>,
    reader: LogReader,
    /// The last head that released anything (or the one resumed from).
    head: Option<SignedHead>,
    /// Whether the reader starts from something (a head, a manifest): a
    /// checkpoint at or behind it is not taken again.
    anchored: bool,
    /// The next bodies released replace what was folded.
    reset: bool,
    synced: bool,
    /// Nothing older than this is released.
    floor: Option<Floor>,
}

impl Follower {
    /// From the start of `stream`, written by the machine whose Ed25519 key
    /// is `machine_key`; `keys`: the machine-key epochs held.
    pub fn new(stream: StreamId, machine_key: [u8; 32], keys: &[(u32, [u8; 32])]) -> Self {
        let keys: Vec<_> = keys.iter().map(|(e, k)| (*e, Zeroizing::new(*k))).collect();
        let mut f = Self {
            stream,
            machine_key,
            reader: LogReader::new(stream, machine_key),
            keys,
            head: None,
            anchored: false,
            reset: true,
            synced: false,
            floor: None,
        };
        f.add_keys();
        f
    }

    /// Release nothing older than `floor` (the copy's last released head,
    /// when the stream is read again from the start).
    pub fn with_floor(mut self, floor: Option<Floor>) -> Self {
        self.floor = match (self.floor, floor) {
            (Some(a), Some(b)) => Some(a.raise(b)),
            (a, b) => a.or(b),
        };
        self
    }

    /// The floor now.
    pub fn floor(&self) -> Option<Floor> {
        self.floor
    }

    fn rolls_back(&self, head: &StreamHead) -> bool {
        self.floor.is_some_and(|f| f.undercut_by(head))
    }

    /// A head released up to. The floor rises with what becomes visible
    /// only (after the catch-up): during it, the reader may pass through
    /// older heads of an honest hub on the way to its newest.
    fn released(&mut self, head: SignedHead) {
        if self.synced {
            self.raise_floor(&head);
        }
        self.head = Some(head);
        self.anchored = true;
    }

    fn raise_floor(&mut self, head: &SignedHead) {
        let floor = Floor::of(head);
        self.floor = Some(self.floor.map_or(floor, |f| f.raise(floor)));
    }

    /// Carry on after `head`, a head this reader released up to before
    /// (checked again here: it must be `stream`'s, signed by the machine).
    pub fn resume(stream: StreamId, machine_key: [u8; 32], keys: &[(u32, [u8; 32])], head: SignedHead) -> Result<Self, FollowError> {
        Self::resume_stored(stream, machine_key, &machine_key, &[], keys, head)
    }

    /// [`Follower::resume`] from a position this device stored, after the
    /// machine may have renewed its identity key
    /// (`docs/host-identity-rotation.md` §5.4): `head` is checked under
    /// exactly `signed_by`, the machine's key this device verified it under
    /// when it stored it — which must be the machine's key now or one the
    /// chain says it held (`retired`: its `retired_keys`, what `keys_at`
    /// reads), never any key that happens to verify. Everything that comes
    /// from the network after it is checked under `machine_key` only:
    /// nothing a retired key signs anew is taken.
    pub fn resume_stored(
        stream: StreamId,
        machine_key: [u8; 32],
        signed_by: &[u8; 32],
        retired: &[[u8; 32]],
        keys: &[(u32, [u8; 32])],
        head: SignedHead,
    ) -> Result<Self, FollowError> {
        if head.head.stream != stream {
            return Err(broken("a cursor of another stream"));
        }
        if *signed_by != machine_key && !retired.contains(signed_by) {
            return Err(broken("a cursor stored under a key the chain does not say the machine held"));
        }
        if !head.verify(signed_by) {
            return Err(broken("a cursor its recorded key did not sign"));
        }
        let reader = LogReader::from_stored_head(machine_key, signed_by, &head).map_err(broken)?;
        let mut f = Self::new(stream, machine_key, keys);
        f.reader = reader;
        f.add_keys();
        f.raise_floor(&head);
        f.released(head);
        f.reset = false;
        Ok(f)
    }

    fn add_keys(&mut self) {
        for (epoch, mk) in &self.keys {
            self.reader.add_epoch(*epoch, mk);
        }
    }

    /// Where to subscribe from (`SUB from_seq`).
    pub fn from_seq(&self) -> u64 {
        self.reader.confirmed_next()
    }

    /// The cursor: the newest machine-signed head released up to.
    pub fn head(&self) -> Option<&SignedHead> {
        self.head.as_ref()
    }

    /// The stream followed.
    pub fn stream(&self) -> StreamId {
        self.stream
    }

    /// Whether the catch-up ended.
    pub fn synced(&self) -> bool {
        self.synced
    }

    /// The time (ms) of the newest head the machine signed for this stream.
    pub fn freshest_ms(&self) -> Option<u64> {
        self.reader.freshest_ms()
    }

    /// What `frame` (on this subscription's route) means.
    pub fn on_frame(&mut self, frame: Frame) -> Result<Step, FollowError> {
        match frame {
            Frame::Unknown {
                ty: CONTROL_FRAME, payload, ..
            } => {
                let v: Value = serde_json::from_slice(&payload).map_err(broken)?;
                match v["t"].as_str() {
                    Some("stream_synced") => {
                        self.synced = true;
                        if v["exists"] != true {
                            return Ok(Step::Missing);
                        }
                        // What the catch-up reached is what becomes visible:
                        // not older than the copy already seen.
                        if let Some(floor) = self.floor {
                            match &self.head {
                                Some(head) if !floor.undercut_by(&head.head) => {}
                                _ => {
                                    return Err(broken(format!(
                                        "the hub's newest is behind the copy (seq {}): withheld or rolled back, refused",
                                        floor.next_seq
                                    )))
                                }
                            }
                        }
                        if let Some(head) = self.head.clone() {
                            self.raise_floor(&head);
                        }
                        Ok(Step::Synced)
                    }
                    Some("stream_refused") => Err(FollowError::Refused(v["code"].as_str().unwrap_or("unknown").to_string())),
                    _ => Ok(Step::Nothing),
                }
            }
            Frame::Gone { first_seq, .. } => {
                // Only records past where the reader stands can be gone from
                // under it; anything else is the hub moving it back.
                if first_seq <= self.reader.confirmed_next() {
                    return Err(broken(format!(
                        "GONE from {first_seq}, at or before {} already released: refused",
                        self.reader.confirmed_next()
                    )));
                }
                self.reader = LogReader::new(self.stream, self.machine_key);
                self.add_keys();
                self.head = None;
                self.anchored = false;
                self.reset = true;
                Ok(Step::Nothing)
            }
            Frame::Checkpoint { manifest, upto_seq, .. } => {
                let manifest = Manifest::decode(&manifest).map_err(broken)?;
                if manifest.upto_seq() != upto_seq || manifest.stream() != self.stream {
                    return Err(broken("a checkpoint frame whose manifest stands elsewhere"));
                }
                // The machine's signature first: no page is fetched for a manifest it did not sign.
                if !manifest.verify(&self.machine_key) {
                    return Err(broken("a manifest the machine did not sign"));
                }
                // A compaction behind what is held already: nothing new in it.
                if self.anchored && self.reader.confirmed_next() >= upto_seq {
                    return Ok(Step::Nothing);
                }
                if self.synced && self.rolls_back(&manifest.head.head) {
                    return Err(broken("a checkpoint older than the copy: refused"));
                }
                Ok(Step::Checkpoint(Box::new(manifest)))
            }
            Frame::Recs { records, .. } => {
                for record in &records {
                    self.reader.push(record).map_err(|e| broken(format!("record {}: {e}", record.seq)))?;
                }
                Ok(Step::Nothing)
            }
            Frame::Head { head, .. } => {
                let head = SignedHead::decode(&head).map_err(broken)?;
                let before = self.reader.confirmed_next();
                // After the catch-up, a head is visible at once: not older than the copy.
                if self.synced && head.verify(&self.machine_key) && self.rolls_back(&head.head) {
                    return Err(broken("a head older than the copy: refused"));
                }
                match self.reader.confirm(&head) {
                    Ok(released) => {
                        // A head older than what is released moves nothing.
                        if head.head.next_seq < before {
                            return Ok(Step::Nothing);
                        }
                        self.released(head);
                        Ok(Step::Released {
                            reset: std::mem::take(&mut self.reset),
                            bodies: released.into_iter().map(|o| (o.fmt, o.body)).collect(),
                        })
                    }
                    Err(rh_proto::record::RecordError::Incomplete { .. }) => Err(broken("the hub's head covers records it did not send")),
                    Err(e) => Err(broken(format!("a head the records do not match: {e}"))),
                }
            }
            _ => Ok(Step::Nothing),
        }
    }

    /// Start from `manifest` ([`Step::Checkpoint`]) with its pages, as
    /// fetched, in its order: each must be the one the signed manifest names
    /// and open under a key held. Its pages are released at once (the
    /// manifest is the machine's signature over them), replacing everything.
    pub fn checkpoint(&mut self, manifest: &Manifest, pages: &[Vec<u8>]) -> Result<Step, FollowError> {
        if manifest.stream() != self.stream {
            return Err(broken("a manifest of another stream"));
        }
        if pages.len() != manifest.pages.len() {
            return Err(broken("not every page of the manifest"));
        }
        if self.synced && self.rolls_back(&manifest.head.head) {
            return Err(broken("a checkpoint older than the copy: refused"));
        }
        let mut reader = reader_from_manifest(self.machine_key, manifest).map_err(broken)?;
        let mut bodies = Vec::with_capacity(pages.len());
        for (i, data) in pages.iter().enumerate() {
            check_page(manifest, i, data)?;
            let page = SealedPage::decode(data).map_err(broken)?;
            let mk = self
                .keys
                .iter()
                .find(|(e, _)| *e == page.epoch)
                .map(|(_, k)| k)
                .ok_or_else(|| broken(format!("no key for epoch {}", page.epoch)))?;
            let key = rh_proto::record::stream_key(mk, &self.stream);
            let opened = open_page(&key, &self.stream, &page).map_err(broken)?;
            bodies.push((opened.fmt, opened.body));
        }
        for (epoch, mk) in &self.keys {
            reader.add_epoch(*epoch, mk);
        }
        self.reader = reader;
        self.released(manifest.head.clone());
        self.reset = false;
        Ok(Step::Released { reset: true, bodies })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;
    use rh_proto::record::{Compression, LogWriter, SealedRecord, FMT_INDEX_V1};
    use rh_proto::stream::seal_page;
    use serde_json::json;

    const MK: [u8; 32] = [0x11; 32];

    fn machine() -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&[7; 32])
    }

    fn stream() -> StreamId {
        rh_proto::stream::index_stream(&MK)
    }

    fn body(i: u64) -> Vec<u8> {
        serde_json::to_vec(&json!({"upserts": [{"source": "claude", "id": format!("s{i}")}], "removed": [], "asOf": i})).unwrap()
    }

    /// A machine writing the index: `n` records, and the head after them.
    struct Machine {
        writer: LogWriter,
        records: Vec<SealedRecord>,
    }

    impl Machine {
        fn new() -> Self {
            Self {
                writer: LogWriter::new(&MK, 0, stream(), 0, [0; 32]),
                records: Vec::new(),
            }
        }

        fn write(&mut self, n: u64) -> Vec<SealedRecord> {
            let start = self.records.len() as u64;
            let new: Vec<SealedRecord> = (start..start + n)
                .map(|i| self.writer.append(FMT_INDEX_V1, &body(i), Compression::None, &mut OsRng).unwrap())
                .collect();
            self.records.extend(new.iter().cloned());
            new
        }

        fn head(&self) -> SignedHead {
            self.writer.head(&machine(), 1000 + self.writer.next_seq())
        }
    }

    fn recs(records: Vec<SealedRecord>) -> Frame {
        Frame::Recs {
            ch: 1,
            stream: stream(),
            records,
        }
    }

    fn head(h: &SignedHead) -> Frame {
        Frame::Head { ch: 1, head: h.encode() }
    }

    fn synced(exists: bool) -> Frame {
        Frame::Unknown {
            ty: CONTROL_FRAME,
            ch: 1,
            payload: json!({"t": "stream_synced", "ch": 1, "exists": exists}).to_string().into_bytes(),
        }
    }

    fn ids(bodies: &[(u16, Vec<u8>)]) -> Vec<u64> {
        bodies
            .iter()
            .map(|(_, b)| serde_json::from_slice::<Value>(b).unwrap()["asOf"].as_u64().unwrap())
            .collect()
    }

    fn released(step: Step) -> (bool, Vec<u64>) {
        match step {
            Step::Released { reset, bodies } => (reset, ids(&bodies)),
            other => panic!("not a release: {other:?}"),
        }
    }

    #[test]
    fn only_what_a_signed_head_covers_is_released() {
        let mut m = Machine::new();
        let mut f = Follower::new(stream(), machine().verifying_key().to_bytes(), &[(0, MK)]);
        let first = m.write(3);
        assert_eq!(f.on_frame(recs(first)).unwrap(), Step::Nothing, "records alone release nothing");
        let h = m.head();
        assert_eq!(released(f.on_frame(head(&h)).unwrap()), (true, vec![0, 1, 2]));
        assert_eq!(f.on_frame(synced(true)).unwrap(), Step::Synced);
        assert_eq!(f.head(), Some(&h));
        assert_eq!(f.from_seq(), 3);
        // Two more, but a head that covers only the first of them was not signed:
        // a head beyond what arrived is refused, and nothing is released.
        let more = m.write(2);
        f.on_frame(recs(more[..1].to_vec())).unwrap();
        let ahead = m.head();
        assert!(matches!(f.on_frame(head(&ahead)), Err(FollowError::Broken(_))));
        // A head someone else signed is refused too.
        let mut forged = m.head();
        forged.signature[0] ^= 1;
        let mut g = Follower::new(stream(), machine().verifying_key().to_bytes(), &[(0, MK)]);
        g.on_frame(recs(m.records.clone())).unwrap();
        assert!(g.on_frame(head(&forged)).is_err());
        // No stream yet.
        let mut h2 = Follower::new(stream(), machine().verifying_key().to_bytes(), &[(0, MK)]);
        assert_eq!(h2.on_frame(synced(false)).unwrap(), Step::Missing);
    }

    #[test]
    fn a_follower_carries_on_from_its_cursor() {
        let mut m = Machine::new();
        let key = machine().verifying_key().to_bytes();
        let mut f = Follower::new(stream(), key, &[(0, MK)]);
        f.on_frame(recs(m.write(2))).unwrap();
        f.on_frame(head(&m.head())).unwrap();
        let cursor = f.head().cloned().unwrap();
        drop(f);
        // Later: the machine wrote on; a new follower resumes from the cursor alone.
        let more = m.write(2);
        let mut g = Follower::resume(stream(), key, &[(0, MK)], cursor.clone()).unwrap();
        assert_eq!(g.from_seq(), 2);
        g.on_frame(recs(more)).unwrap();
        assert_eq!(released(g.on_frame(head(&m.head())).unwrap()), (false, vec![2, 3]));
        // A heartbeat at the same place releases nothing and keeps the cursor current.
        let beat = m.writer.head(&machine(), 99_999);
        assert_eq!(released(g.on_frame(head(&beat)).unwrap()), (false, vec![]));
        assert_eq!(g.freshest_ms(), Some(99_999));
        // A cursor that is not the machine's, or not this stream's, is refused.
        let mut bad = cursor.clone();
        bad.signature[3] ^= 1;
        assert!(Follower::resume(stream(), key, &[(0, MK)], bad).is_err());
        let other = rh_proto::stream::status_stream(&MK);
        assert!(Follower::resume(other, key, &[(0, MK)], cursor).is_err());
    }

    /// Host identity renewal (`docs/host-identity-rotation.md` §5.4, §8
    /// item 9): a head or manifest from the network is taken under the
    /// machine's current key only — one its retired key signs is refused,
    /// even where it would link — while a position this device stored, signed
    /// by the retired key when it was current, still resumes (the chain's
    /// `keys_at`), and what follows it must be the new key's.
    #[test]
    fn after_a_renewal_only_the_new_key_signs_what_comes_from_the_network() {
        let old = machine();
        let new = ed25519_dalek::SigningKey::from_bytes(&[8; 32]);
        let (old_key, new_key) = (old.verifying_key().to_bytes(), new.verifying_key().to_bytes());
        let mut m = Machine::new();
        // Stored before the renewal: a head the old key signed.
        let mut f = Follower::new(stream(), old_key, &[(0, MK)]);
        f.on_frame(recs(m.write(2))).unwrap();
        f.on_frame(head(&m.head())).unwrap();
        let cursor = f.head().cloned().unwrap();
        // From the network, under the new key: the old key's head is refused…
        let more = m.write(2);
        let mut fresh = Follower::new(stream(), new_key, &[(0, MK)]);
        fresh.on_frame(recs(m.records.clone())).unwrap();
        assert!(fresh.on_frame(head(&m.writer.head(&old, 5_000))).is_err(), "a retired key's head");
        // …and so is a manifest it signs (before any page is fetched).
        let stale = Manifest::sign(m.writer.head(&old, 5_000), vec![], &old).unwrap();
        let mut g = Follower::new(stream(), new_key, &[(0, MK)]);
        let checkpoint = |manifest: &Manifest| Frame::Checkpoint {
            ch: 1,
            stream: stream(),
            upto_seq: 4,
            manifest: manifest.encode(),
        };
        assert!(g.on_frame(checkpoint(&stale)).is_err(), "a retired key's manifest");
        let signed = Manifest::sign(m.writer.head(&new, 5_000), vec![], &new).unwrap();
        assert!(matches!(g.on_frame(checkpoint(&signed)), Ok(Step::Checkpoint(_))), "the new key's");
        // The stored position: not under the new key alone…
        assert!(Follower::resume(stream(), new_key, &[(0, MK)], cursor.clone()).is_err());
        // …but under the key it was stored under, one the machine held (keys_at), and on from there under the new one only.
        let mut r = Follower::resume_stored(stream(), new_key, &old_key, &[old_key], &[(0, MK)], cursor.clone()).unwrap();
        assert_eq!(r.from_seq(), 2);
        r.on_frame(recs(more.clone())).unwrap();
        assert!(r.on_frame(head(&m.writer.head(&old, 6_000))).is_err(), "nothing new under the retired key");
        let mut r = Follower::resume_stored(stream(), new_key, &old_key, &[old_key], &[(0, MK)], cursor.clone()).unwrap();
        r.on_frame(recs(more)).unwrap();
        assert_eq!(released(r.on_frame(head(&m.writer.head(&new, 6_000))).unwrap()), (false, vec![2, 3]));
        // Only the exact key it was stored under (R6 review): not any retired
        // key that happens to verify, nor a recorded key the machine never held.
        assert!(
            Follower::resume_stored(stream(), new_key, &new_key, &[old_key], &[(0, MK)], cursor.clone()).is_err(),
            "recorded under the new key, signed by the old"
        );
        let stranger = ed25519_dalek::SigningKey::from_bytes(&[9; 32]).verifying_key().to_bytes();
        assert!(Follower::resume_stored(stream(), new_key, &old_key, &[stranger], &[(0, MK)], cursor.clone()).is_err(), "a key it never held");
        assert!(Follower::resume_stored(stream(), new_key, &stranger, &[stranger], &[(0, MK)], cursor).is_err());
    }

    #[test]
    fn gone_and_a_checkpoint_rebuild_the_copy_from_the_manifest() {
        let mut m = Machine::new();
        let key = machine().verifying_key().to_bytes();
        let mut f = Follower::new(stream(), key, &[(0, MK)]);
        f.on_frame(recs(m.write(2))).unwrap();
        f.on_frame(head(&m.head())).unwrap();
        // The machine compacts 0..4 into one page; the hub trims the tail.
        m.write(2);
        let state = serde_json::to_vec(&json!({"reset": true, "upserts": [], "removed": [], "asOf": 40})).unwrap();
        let stream_key = rh_proto::record::stream_key(&MK, &stream());
        let page = seal_page(&stream_key, &stream(), 0, FMT_INDEX_V1, &state, Compression::None, &mut OsRng)
            .unwrap()
            .encode();
        let manifest = Manifest::sign(m.head(), vec![page_hash(&page)], &machine()).unwrap();
        let after = m.write(1);
        assert_eq!(
            f.on_frame(Frame::Gone {
                ch: 1,
                stream: stream(),
                first_seq: 4
            })
            .unwrap(),
            Step::Nothing
        );
        let step = f
            .on_frame(Frame::Checkpoint {
                ch: 1,
                stream: stream(),
                upto_seq: 4,
                manifest: manifest.encode(),
            })
            .unwrap();
        let Step::Checkpoint(got) = step else { panic!("{step:?}") };
        // A page that is not the one the manifest names is refused.
        let mut wrong = page.clone();
        *wrong.last_mut().unwrap() ^= 1;
        assert!(f.checkpoint(&got, std::slice::from_ref(&wrong)).is_err());
        assert_eq!(released(f.checkpoint(&got, std::slice::from_ref(&page)).unwrap()), (true, vec![40]));
        f.on_frame(recs(after)).unwrap();
        assert_eq!(released(f.on_frame(head(&m.head())).unwrap()), (false, vec![4]));
        // The same checkpoint again (a later compaction behind the reader): nothing.
        assert_eq!(
            f.on_frame(Frame::Checkpoint {
                ch: 1,
                stream: stream(),
                upto_seq: 4,
                manifest: manifest.encode()
            })
            .unwrap(),
            Step::Nothing
        );
    }

    #[test]
    fn a_bad_record_breaks_the_follower() {
        let mut m = Machine::new();
        let key = machine().verifying_key().to_bytes();
        let mut f = Follower::new(stream(), key, &[(0, MK)]);
        let mut records = m.write(2);
        records[1].ct[0] ^= 1;
        assert!(matches!(f.on_frame(recs(records)), Err(FollowError::Broken(_))));
        // Out of order, or under a key it does not hold.
        let mut g = Follower::new(stream(), key, &[(0, MK)]);
        assert!(g.on_frame(recs(m.records[1..].to_vec())).is_err());
        let mut h = Follower::new(stream(), key, &[(5, [9; 32])]);
        assert!(h.on_frame(recs(m.records.clone())).is_err());
        // A refusal says which.
        let mut r = Follower::new(stream(), key, &[(0, MK)]);
        let refused = Frame::Unknown {
            ty: CONTROL_FRAME,
            ch: 1,
            payload: json!({"t": "stream_refused", "ch": 1, "code": "limit"}).to_string().into_bytes(),
        };
        assert_eq!(r.on_frame(refused), Err(FollowError::Refused("limit".into())));
    }

    /// F1: a manifest the machine did not sign is refused when it arrives —
    /// before the caller fetches any of the pages it lists — and each page is
    /// checked against the manifest as it comes.
    #[test]
    fn a_forged_checkpoint_is_refused_before_any_page_is_fetched() {
        let mut m = Machine::new();
        m.write(2);
        let key = machine().verifying_key().to_bytes();
        let stranger = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let pages: Vec<[u8; 32]> = (0..1024u32).map(|i| [i as u8; 32]).collect();
        let forged = Manifest::sign(m.writer.head(&stranger, 5), pages, &stranger).unwrap();
        let mut f = Follower::new(stream(), key, &[(0, MK)]);
        let frame = Frame::Checkpoint {
            ch: 1,
            stream: stream(),
            upto_seq: 2,
            manifest: forged.encode(),
        };
        assert!(matches!(f.on_frame(frame), Err(FollowError::Broken(why)) if why.contains("did not sign")));
        // A genuine manifest: its pages are checked one by one.
        let genuine = Manifest::sign(m.head(), vec![page_hash(b"page zero"), page_hash(b"page one")], &machine()).unwrap();
        assert!(check_page(&genuine, 0, b"page zero").is_ok());
        assert!(check_page(&genuine, 1, b"page zero").is_err(), "not the page it names");
        assert!(check_page(&genuine, 2, b"page one").is_err(), "past the list");
    }

    fn synced_frame() -> Frame {
        synced(true)
    }

    /// F2 and its re-review (N1): once a copy has been seen up to a head,
    /// the hub cannot move it back with the machine's own older signatures —
    /// not by GONE, and not by stopping a re-read below it (the caller keeps
    /// the copy it has) — while an honest re-read, whose newest checkpoint is
    /// older than the copy but whose records and newest head are not, is
    /// taken; and a stream the machine wrote again from 0 later is read.
    #[test]
    fn a_hub_cannot_roll_a_copy_back() {
        let key = machine().verifying_key().to_bytes();
        let sealed = |b: &[u8]| {
            let sk = rh_proto::record::stream_key(&MK, &stream());
            seal_page(&sk, &stream(), 0, FMT_INDEX_V1, b, Compression::None, &mut OsRng)
                .unwrap()
                .encode()
        };
        let mut m = Machine::new();
        let early = m.write(3);
        let old_head = m.head(); // seq 3, time 1003
        let page = sealed(&body(2));
        let old_manifest = Manifest::sign(old_head.clone(), vec![page_hash(&page)], &machine()).unwrap();
        m.write(5);
        let compacted = m.head(); // seq 8, time 1008: the hub's newest checkpoint
        let page8 = sealed(&body(7));
        let manifest8 = Manifest::sign(compacted.clone(), vec![page_hash(&page8)], &machine()).unwrap();
        m.write(2);
        let new_head = m.head(); // seq 10, time 1010: what the copy was seen at
        let floor = Floor::of(&new_head);
        let after = m.write(2);
        let newest = m.head(); // seq 12, time 1012
        let checkpoint = |manifest: &Manifest| Frame::Checkpoint {
            ch: 1,
            stream: stream(),
            upto_seq: manifest.upto_seq(),
            manifest: manifest.encode(),
        };
        let gone = |first_seq| Frame::Gone {
            ch: 1,
            stream: stream(),
            first_seq,
        };

        // Resumed at 10: GONE from 3, or 10 (at or before what is held), is refused.
        let mut f = Follower::resume(stream(), key, &[(0, MK)], new_head.clone()).unwrap();
        assert_eq!(f.floor(), Some(floor));
        assert!(f.on_frame(gone(3)).is_err());
        assert!(f.on_frame(gone(10)).is_err());

        // N1: an honest re-read from 0 — the newest checkpoint (8) is behind
        // the copy (10), the records and the newest head (12) are not: taken.
        let mut honest = Follower::new(stream(), key, &[(0, MK)]).with_floor(Some(floor));
        let Step::Checkpoint(got) = honest.on_frame(checkpoint(&manifest8)).unwrap() else {
            panic!("a checkpoint")
        };
        assert_eq!(released(honest.checkpoint(&got, std::slice::from_ref(&page8)).unwrap()), (true, vec![7]));
        honest.on_frame(recs(m.records[8..12].to_vec())).unwrap();
        assert_eq!(released(honest.on_frame(head(&newest)).unwrap()), (false, vec![8, 9, 10, 11]));
        assert_eq!(honest.on_frame(synced_frame()).unwrap(), Step::Synced);
        assert_eq!(honest.floor(), Some(Floor::of(&newest)), "the floor rises with what became visible");
        let _ = after;

        // A re-read the hub stops below the copy — an older checkpoint and
        // nothing after it, or older records and their head: refused at the
        // end of the catch-up, so the caller keeps the copy it has.
        let mut g = Follower::new(stream(), key, &[(0, MK)]).with_floor(Some(floor));
        let Step::Checkpoint(old) = g.on_frame(checkpoint(&old_manifest)).unwrap() else {
            panic!()
        };
        g.checkpoint(&old, std::slice::from_ref(&page)).unwrap();
        assert!(matches!(g.on_frame(synced_frame()), Err(FollowError::Broken(why)) if why.contains("behind the copy")));
        let mut h = Follower::new(stream(), key, &[(0, MK)]).with_floor(Some(floor));
        h.on_frame(recs(early.clone())).unwrap();
        h.on_frame(head(&old_head)).unwrap();
        assert!(h.on_frame(synced_frame()).is_err());
        // Nothing released at all, with a copy held: withheld.
        let mut nothing = Follower::new(stream(), key, &[(0, MK)]).with_floor(Some(floor));
        assert!(nothing.on_frame(synced_frame()).is_err());
        // Without a floor the same frames read (a first read).
        let mut first = Follower::new(stream(), key, &[(0, MK)]);
        first.on_frame(recs(early)).unwrap();
        assert_eq!(released(first.on_frame(head(&old_head)).unwrap()), (true, vec![0, 1, 2]));
        assert_eq!(first.on_frame(synced_frame()).unwrap(), Step::Synced);

        // After the catch-up, an older checkpoint is refused at once.
        let mut live = Follower::resume(stream(), key, &[(0, MK)], new_head.clone()).unwrap();
        assert_eq!(live.on_frame(synced_frame()).unwrap(), Step::Synced);
        assert!(live.on_frame(gone(15)).is_ok(), "records past it gone: taken");
        assert!(matches!(live.on_frame(checkpoint(&manifest8)), Err(FollowError::Broken(why)) if why.contains("older than the copy")));

        // The machine deleted the stream and wrote it again from 0, later: read.
        let mut again = LogWriter::new(&MK, 0, stream(), 0, [0; 32]);
        let rewritten: Vec<SealedRecord> = (0..2)
            .map(|i| again.append(FMT_INDEX_V1, &body(100 + i), Compression::None, &mut OsRng).unwrap())
            .collect();
        let later = again.head(&machine(), 5_000);
        let mut r = Follower::new(stream(), key, &[(0, MK)]).with_floor(Some(floor));
        r.on_frame(recs(rewritten.clone())).unwrap();
        assert_eq!(released(r.on_frame(head(&later)).unwrap()), (true, vec![100, 101]));
        assert_eq!(r.on_frame(synced_frame()).unwrap(), Step::Synced);
        assert_eq!(
            r.floor(),
            Some(Floor {
                next_seq: 10,
                time_ms: 5_000
            }),
            "the floor only rises"
        );

        // N3: a machine whose clock ran a lifetime ahead set the copy at
        // (50, year 2100); once its clock is right it rewrites the stream from
        // 0 and is still read.
        let now = crate::now_ms();
        let mut skewed = LogWriter::new(&MK, 0, stream(), 0, [0; 32]);
        for i in 0..50 {
            skewed.append(FMT_INDEX_V1, &body(i), Compression::None, &mut OsRng).unwrap();
        }
        let year_2100 = 4_102_444_800_000;
        let ahead = Floor::of(&skewed.head(&machine(), year_2100));
        assert_eq!(ahead.next_seq, 50);
        assert!(ahead.time_ms <= now + 1_000, "the far-ahead time counts as now");
        let fixed = again.head(&machine(), now + 60_000); // (2, a minute from now)
        let mut back = Follower::new(stream(), key, &[(0, MK)]).with_floor(Some(ahead));
        back.on_frame(recs(rewritten)).unwrap();
        back.on_frame(head(&fixed)).unwrap();
        assert_eq!(back.on_frame(synced_frame()).unwrap(), Step::Synced);
        assert_eq!(
            Floor::at(&fixed, now),
            Floor {
                next_seq: 2,
                time_ms: now + 60_000
            },
            "an hour ahead or less is taken as signed"
        );
    }
}
