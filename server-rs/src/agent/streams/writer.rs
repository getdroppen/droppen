//! The agent's side of its streams, sans IO: seal what the sync task decides
//! to write, keep at most one request per stream in flight, and fold the
//! hub's answers into [`super::store`] — always the position and the cursor
//! together, in one transaction, when the `ACK` is handled.
//!
//! - **Append**: the records are written ahead ([`Store::write_ahead`]) and
//!   then sent as `APPEND {ch: request, expect_seq}`. An `ACK` commits them;
//!   a `CONFLICT` means the hub's stream is not where this agent thought it
//!   was (it lost records, or holds ones this agent forgot), and the sync task
//!   rebuilds the stream from the machine's own truth with a rebase. After a
//!   reconnect the written-ahead bytes are sent again as they were, which the
//!   hub acknowledges whether or not it had committed them.
//! - **Checkpoint**: new pages go up as `PAGE` frames, then `CHECKPOINT` with
//!   a manifest signed over a head at `upto` — this agent's own position for a
//!   compaction (same link: readers holding the records carry on), or a fresh
//!   link for a rebase. Its `ACK` commits the new position, cursor and page
//!   list together.
//! - **Heads**: one after every acknowledged append, and as heartbeats.

use std::collections::{BTreeMap, HashMap};

use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use rand::RngCore;
use rh_proto::frame::{Frame, MAX_PAYLOAD_LEN};
use rh_proto::record::{stream_key, Compression, LogWriter, SealedRecord, StreamId};
use rh_proto::stream::{seal_page, Manifest};
use zeroize::Zeroizing;

use super::store::{IndexDiff, PageRef, Pending, Row, Store};

/// What the records of one `APPEND` may add up to.
const APPEND_BUDGET: usize = MAX_PAYLOAD_LEN - 32 - 8 - 2;

/// One body to append, with the cursor the stream stands at once it is in.
#[derive(Debug, Clone)]
pub struct Step {
    pub fmt: u16,
    pub body: Vec<u8>,
    pub cursor: String,
    /// Index streams: the rows this body changes.
    pub diff: Option<IndexDiff>,
}

/// A page of a checkpoint: a page the last manifest already named, or a new body.
#[derive(Debug, Clone)]
pub enum PageSpec {
    Reuse(PageRef),
    New {
        body: Vec<u8>,
        from: usize,
        to: usize,
        fp: String,
        /// The newest dated entry it holds (session pages).
        last_ms: Option<u64>,
        /// A session copy's window-start page ([`super::content::window_body`]).
        window: bool,
    },
}

impl PageSpec {
    /// A page that is only a body (index, status).
    pub fn body(body: Vec<u8>) -> Self {
        PageSpec::New {
            body,
            from: 0,
            to: 0,
            fp: String::new(),
            last_ms: None,
            window: false,
        }
    }
}

/// A checkpoint to write.
#[derive(Debug, Clone)]
pub struct Plan {
    pub fmt: u16,
    pub pages: Vec<PageSpec>,
    /// Where the manifest stands: this agent's position (compaction), or past
    /// the hub's head (rebase).
    pub upto: u64,
    /// `None`: keep the stream's link (compaction); `Some`: a fresh one (rebase).
    pub fresh_link: bool,
    pub cursor: String,
    pub state_bytes: i64,
    /// Index streams: the rows the pages hold.
    pub rows: Option<Vec<(String, String)>>,
}

enum InFlight {
    Append {
        stream: StreamId,
    },
    Checkpoint {
        stream: StreamId,
        epoch: u32,
        next_seq: u64,
        link: [u8; 32],
        cursor: String,
        pages: Vec<PageRef>,
        state_bytes: i64,
        rows: Option<Vec<(String, String)>>,
    },
    /// The stream's manifest as it stands, signed again ([`Writer::resign`]).
    Resign {
        stream: StreamId,
    },
}

/// What an answer from the hub came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// An append or a checkpoint is committed; the head frame to send follows.
    Committed { stream: StreamId, checkpoint: bool },
    /// A re-signed manifest is committed: nothing of the stream moved.
    Resigned { stream: StreamId },
    /// The hub's stream is elsewhere (`head`): rebuild it.
    Conflict { stream: StreamId, head: u64 },
    /// Not ours (an old request from a connection that has gone).
    Unknown,
}

pub struct Writer {
    pub store: Store,
    keys: BTreeMap<u32, Zeroizing<[u8; 32]>>,
    signing: SigningKey,
    inflight: HashMap<u32, InFlight>,
    busy: HashMap<StreamId, u32>,
    next_req: u32,
}

impl Writer {
    pub fn new(store: Store, signing: SigningKey) -> Self {
        Self {
            store,
            keys: BTreeMap::new(),
            signing,
            inflight: HashMap::new(),
            busy: HashMap::new(),
            next_req: 1,
        }
    }

    /// Learn machine key `epoch`.
    pub fn add_key(&mut self, epoch: u32, mk: Zeroizing<[u8; 32]>) {
        self.keys.insert(epoch, mk);
    }

    /// The newest epoch held, and its key.
    pub fn current(&self) -> Option<(u32, &[u8; 32])> {
        self.keys.iter().next_back().map(|(e, k)| (*e, &**k))
    }

    /// The first epoch held, and its key: what every stream was named under
    /// before `streams/2` (relay-and-plans §5), and what a stream with no
    /// name epoch of its own still is.
    pub fn first(&self) -> Option<(u32, &[u8; 32])> {
        self.keys.iter().next().map(|(e, k)| (*e, &**k))
    }

    /// The machine's signing key (heads, manifests, drafts).
    pub fn signing(&self) -> &SigningKey {
        &self.signing
    }

    pub fn key(&self, epoch: u32) -> Option<&[u8; 32]> {
        self.keys.get(&epoch).map(|k| &**k)
    }

    /// Every machine key held, oldest first.
    pub fn keys(&self) -> impl Iterator<Item = (u32, &[u8; 32])> {
        self.keys.iter().map(|(e, k)| (*e, &**k))
    }

    /// Whether a request for `stream` is in flight.
    pub fn busy(&self, stream: &StreamId) -> bool {
        self.busy.contains_key(stream)
    }

    pub fn in_flight(&self) -> usize {
        self.inflight.len()
    }

    /// The connection went: nothing in flight will be answered.
    pub fn disconnected(&mut self) {
        self.inflight.clear();
        self.busy.clear();
    }

    fn request(&mut self, stream: StreamId, what: InFlight) -> u32 {
        let ch = self.next_req;
        self.next_req = self.next_req.wrapping_add(1).max(1);
        self.inflight.insert(ch, what);
        self.busy.insert(stream, ch);
        ch
    }

    /// A writer that seals new records in the newest epoch, carrying on from
    /// `row`'s position: a new machine key changes which key the next record
    /// is sealed under, never the chain it extends.
    fn writer(&self, row: &Row) -> Option<LogWriter> {
        let (epoch, mk) = self.current()?;
        Some(LogWriter::new(mk, epoch, row.stream, row.next_seq, row.link))
    }

    /// Seal as many of `steps` as fit one `APPEND`, write them ahead, and
    /// return the frame — or `None` when nothing could be sealed. The rest
    /// waits for the next round, after this one's `ACK`.
    pub fn append(&mut self, row: &Row, steps: &[Step]) -> Result<Option<Frame>, String> {
        if self.busy(&row.stream) || row.pending.is_some() || steps.is_empty() {
            return Ok(None);
        }
        let mut w = self.writer(row).ok_or_else(|| format!("no machine key for epoch {}", row.epoch))?;
        let mut records: Vec<SealedRecord> = Vec::new();
        let mut size = 0;
        let mut cursor = String::new();
        let mut diff = IndexDiff::default();
        let mut has_diff = false;
        for step in steps {
            let mut probe = w.clone();
            let sealed = probe
                .append(step.fmt, &step.body, Compression::None, &mut OsRng)
                .map_err(|e| e.to_string())?;
            let len = sealed.ct.len() + 28;
            if !records.is_empty() && size + len > APPEND_BUDGET {
                break;
            }
            size += len;
            w = probe;
            records.push(sealed);
            cursor = step.cursor.clone();
            if let Some(d) = &step.diff {
                has_diff = true;
                diff.removed.retain(|k| !d.upserts.iter().any(|(u, _)| u == k));
                diff.upserts
                    .retain(|(k, _)| !d.removed.contains(k) && !d.upserts.iter().any(|(u, _)| u == k));
                diff.removed.extend(d.removed.iter().cloned());
                diff.upserts.extend(d.upserts.iter().cloned());
            }
        }
        let pending = Pending {
            expect_seq: row.next_seq,
            bytes: records.iter().map(|r| r.ct.len() as i64).sum(),
            records,
            next_seq: w.next_seq(),
            link: w.link(),
            cursor,
            diff: has_diff.then(|| serde_json::to_string(&diff).unwrap_or_default()),
        };
        self.store.write_ahead(&row.stream, &pending).map_err(|e| e.to_string())?;
        let ch = self.request(row.stream, InFlight::Append { stream: row.stream });
        Ok(Some(Frame::Append {
            ch,
            stream: row.stream,
            expect_seq: pending.expect_seq,
            records: pending.records,
        }))
    }

    /// Send a written-ahead append again, byte for byte.
    pub fn resend(&mut self, row: &Row) -> Option<Frame> {
        let pending = row.pending.as_ref()?;
        if self.busy(&row.stream) {
            return None;
        }
        let ch = self.request(row.stream, InFlight::Append { stream: row.stream });
        Some(Frame::Append {
            ch,
            stream: row.stream,
            expect_seq: pending.expect_seq,
            records: pending.records.clone(),
        })
    }

    /// A signed head at `row`'s position, in the epoch of its last record.
    pub fn head(&self, row: &Row, now_ms: u64) -> Option<Frame> {
        let mk = self.key(row.epoch).or_else(|| self.current().map(|(_, k)| k))?;
        let w = LogWriter::new(mk, row.epoch, row.stream, row.next_seq, row.link);
        Some(Frame::Head {
            ch: 0,
            head: w.head(&self.signing, now_ms).encode(),
        })
    }

    /// The `PAGE` frames and the `CHECKPOINT` for `plan`, in the current epoch.
    pub fn checkpoint(&mut self, row: &Row, plan: Plan, now_ms: u64) -> Result<Vec<Frame>, String> {
        if self.busy(&row.stream) {
            return Ok(Vec::new());
        }
        let (epoch, mk) = self.current().ok_or("no machine key")?;
        let mk = *mk;
        let key = stream_key(&mk, &row.stream);
        let mut frames = Vec::new();
        let mut refs = Vec::with_capacity(plan.pages.len());
        for spec in plan.pages {
            match spec {
                // A page sealed in another epoch is not reused in this one.
                PageSpec::Reuse(r) if r.epoch == epoch => refs.push(r),
                PageSpec::Reuse(_) => return Err("a reused page from another epoch".into()),
                PageSpec::New {
                    body,
                    from,
                    to,
                    fp,
                    last_ms,
                    window,
                } => {
                    let page = seal_page(&key, &row.stream, epoch, plan.fmt, &body, Compression::None, &mut OsRng).map_err(|e| e.to_string())?;
                    let hash = page.hash();
                    let data = page.encode();
                    let size = data.len() as u64;
                    frames.push(Frame::Page {
                        ch: 0,
                        stream: row.stream,
                        hash,
                        data,
                    });
                    refs.push(PageRef {
                        hash: crate::agent::hub::hex(&hash),
                        from,
                        to,
                        fp,
                        epoch,
                        window,
                        last_ms,
                        size,
                    });
                }
            }
        }
        let link = if plan.fresh_link {
            let mut l = [0u8; 32];
            OsRng.fill_bytes(&mut l);
            l
        } else {
            row.link
        };
        let head = LogWriter::new(&mk, epoch, row.stream, plan.upto, link).head(&self.signing, now_ms);
        let hashes = refs
            .iter()
            .map(|r| crate::agent::hub::unhex32(&r.hash).ok_or("a bad page hash"))
            .collect::<Result<Vec<_>, _>>()?;
        let manifest = Manifest::sign(head, hashes, &self.signing).map_err(|e| e.to_string())?;
        let ch = self.request(
            row.stream,
            InFlight::Checkpoint {
                stream: row.stream,
                epoch,
                next_seq: plan.upto,
                link,
                cursor: plan.cursor,
                pages: refs,
                state_bytes: plan.state_bytes,
                rows: plan.rows,
            },
        );
        frames.push(Frame::Checkpoint {
            ch,
            stream: row.stream,
            upto_seq: plan.upto,
            manifest: manifest.encode(),
        });
        Ok(frames)
    }

    /// A `CHECKPOINT` that signs `row`'s manifest again under this writer's
    /// key — the same position, link and pages, only the signatures new — for
    /// a stream whose manifest stands at its position (no record after it).
    /// After an identity renewal (`docs/host-identity-rotation.md` §5.4)
    /// readers take a manifest from the hub only under the machine's current
    /// key; this keeps a copy nothing rewrites (outside the sync window,
    /// waiting for retention) readable. `None` when there is nothing to sign
    /// so, or a request for it is in flight.
    pub fn resign(&mut self, row: &Row, now_ms: u64) -> Option<Frame> {
        if self.busy(&row.stream) || row.manifest_upto != Some(row.next_seq) {
            return None;
        }
        let mk = self.key(row.epoch).or_else(|| self.current().map(|(_, k)| k))?;
        let head = LogWriter::new(mk, row.epoch, row.stream, row.next_seq, row.link).head(&self.signing, now_ms);
        let hashes = row
            .pages
            .iter()
            .map(|r| crate::agent::hub::unhex32(&r.hash))
            .collect::<Option<Vec<_>>>()?;
        let manifest = Manifest::sign(head, hashes, &self.signing).ok()?;
        let ch = self.request(row.stream, InFlight::Resign { stream: row.stream });
        Some(Frame::Checkpoint {
            ch,
            stream: row.stream,
            upto_seq: row.next_seq,
            manifest: manifest.encode(),
        })
    }

    /// `ACK {ch, head}`.
    pub fn on_ack(&mut self, ch: u32, head: u64, now_ms: u64) -> Answer {
        let Some(what) = self.inflight.remove(&ch) else { return Answer::Unknown };
        match what {
            InFlight::Resign { stream } => {
                self.busy.remove(&stream);
                Answer::Resigned { stream }
            }
            InFlight::Append { stream } => {
                self.busy.remove(&stream);
                let Some(row) = self.store.get(&stream) else { return Answer::Unknown };
                match row.pending {
                    Some(p) if p.next_seq == head => {
                        if let Err(error) = self.store.commit_pending(&stream, now_ms) {
                            tracing::error!(%error, "cannot record an acknowledged append; it will be re-sent");
                            return Answer::Unknown;
                        }
                        Answer::Committed { stream, checkpoint: false }
                    }
                    // Acknowledged at a position we did not ask for: the hub is elsewhere.
                    _ => {
                        self.store.drop_pending(&stream);
                        Answer::Conflict { stream, head }
                    }
                }
            }
            InFlight::Checkpoint {
                stream,
                epoch,
                next_seq,
                link,
                cursor,
                pages,
                state_bytes,
                rows,
            } => {
                self.busy.remove(&stream);
                // A rebase stands where it said; a compaction leaves the head
                // where the stream's records are, which is ours.
                let _ = head;
                if let Err(error) =
                    self.store
                        .commit_checkpoint(&stream, epoch, next_seq, &link, &cursor, &pages, state_bytes, rows.as_deref(), now_ms)
                {
                    tracing::error!(%error, "cannot record an acknowledged checkpoint");
                    return Answer::Unknown;
                }
                Answer::Committed { stream, checkpoint: true }
            }
        }
    }

    /// `CONFLICT {ch, head}`.
    pub fn on_conflict(&mut self, ch: u32, head: u64) -> Answer {
        let Some(what) = self.inflight.remove(&ch) else { return Answer::Unknown };
        let stream = match what {
            InFlight::Append { stream } => {
                self.store.drop_pending(&stream);
                stream
            }
            InFlight::Checkpoint { stream, .. } | InFlight::Resign { stream } => stream,
        };
        self.busy.remove(&stream);
        Answer::Conflict { stream, head }
    }

    /// A `stream_refused` for request `ch`: the stream it was for (its
    /// written-ahead append stays, to be re-sent after the back-off).
    pub fn on_refused(&mut self, ch: u32) -> Option<StreamId> {
        let what = self.inflight.remove(&ch)?;
        let stream = match what {
            InFlight::Append { stream } | InFlight::Checkpoint { stream, .. } | InFlight::Resign { stream } => stream,
        };
        self.busy.remove(&stream);
        Some(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::super::store::Kind;
    use super::*;
    use rh_proto::record::{self, LogReader, SignedHead};
    use rh_proto::stream::{open_page, reader_from_manifest, SealedPage};

    const MK: [u8; 32] = [0x5a; 32];

    /// The hub, in memory: a CAS log per stream, pages, a manifest, heads —
    /// and the byte-identical retry rule.
    #[derive(Default)]
    struct FakeHub {
        records: HashMap<StreamId, Vec<SealedRecord>>,
        first: HashMap<StreamId, u64>,
        next: HashMap<StreamId, u64>,
        pages: HashMap<[u8; 32], Vec<u8>>,
        manifest: HashMap<StreamId, Manifest>,
        head: HashMap<StreamId, SignedHead>,
        /// Commit, then lose the answer (the connection drops).
        lose_next_answer: bool,
        /// The machine renewed its identity key to this one: the hub (and a
        /// reader) take its signatures only.
        renewed: Option<[u8; 32]>,
    }

    impl FakeHub {
        /// The machine's key as the hub has it.
        fn machine_key(&self) -> [u8; 32] {
            self.renewed.unwrap_or_else(|| signing().verifying_key().to_bytes())
        }

        fn take(&mut self, frame: Frame) -> Option<Frame> {
            let answer = match frame {
                Frame::Append {
                    ch,
                    stream,
                    expect_seq,
                    records,
                } => {
                    let next = *self.next.get(&stream).unwrap_or(&0);
                    let log = self.records.entry(stream).or_default();
                    if next == expect_seq {
                        log.extend(records.iter().cloned());
                        let head = expect_seq + records.len() as u64;
                        self.next.insert(stream, head);
                        Some(Frame::Ack { ch, stream, head })
                    } else if next == expect_seq + records.len() as u64
                        && log.iter().filter(|r| r.seq >= expect_seq).cloned().collect::<Vec<_>>() == records
                    {
                        Some(Frame::Ack { ch, stream, head: next })
                    } else {
                        Some(Frame::Conflict { ch, stream, head: next })
                    }
                }
                Frame::Page { hash, data, .. } => {
                    assert_eq!(rh_proto::stream::page_hash(&data), hash);
                    self.pages.insert(hash, data);
                    None
                }
                Frame::Checkpoint {
                    ch,
                    stream,
                    upto_seq,
                    manifest,
                } => {
                    let m = Manifest::decode(&manifest).unwrap();
                    assert!(m.verify(&self.machine_key()));
                    assert!(m.pages.iter().all(|p| self.pages.contains_key(p)), "pages before the checkpoint");
                    let next = *self.next.get(&stream).unwrap_or(&0);
                    if upto_seq > next {
                        self.records.insert(stream, Vec::new());
                        self.next.insert(stream, upto_seq);
                        self.first.insert(stream, upto_seq);
                    }
                    self.manifest.insert(stream, m.clone());
                    self.head.insert(stream, m.head.clone());
                    Some(Frame::Ack {
                        ch,
                        stream,
                        head: upto_seq.max(next),
                    })
                }
                Frame::Head { head, .. } => {
                    let head = SignedHead::decode(&head).unwrap();
                    self.head.insert(head.head.stream, head);
                    None
                }
                _ => None,
            };
            if self.lose_next_answer && answer.is_some() {
                self.lose_next_answer = false;
                return None;
            }
            answer
        }

        /// What a fresh device reads: the manifest's pages, then the records,
        /// released only under the newest head.
        fn read(&self, stream: StreamId) -> Vec<Vec<u8>> {
            let key_pub = self.machine_key();
            let mut bodies = Vec::new();
            let mut reader = match self.manifest.get(&stream) {
                Some(m) => {
                    let mut r = reader_from_manifest(key_pub, m).unwrap();
                    r.add_epoch(0, &MK);
                    let key = stream_key(&MK, &stream);
                    for hash in &m.pages {
                        let page = SealedPage::decode(&self.pages[hash]).unwrap();
                        bodies.push(open_page(&key, &stream, &page).unwrap().body);
                    }
                    r
                }
                None => {
                    let mut r = LogReader::new(stream, key_pub);
                    r.add_epoch(0, &MK);
                    r
                }
            };
            let from = reader.confirmed_next();
            for rec in self.records.get(&stream).into_iter().flatten().filter(|r| r.seq >= from) {
                reader.push(rec).expect("a continuous chain");
            }
            if let Some(head) = self.head.get(&stream) {
                bodies.extend(reader.confirm(head).expect("the head covers the records").into_iter().map(|o| o.body));
            }
            bodies
        }
    }

    fn signing() -> SigningKey {
        SigningKey::from_bytes(&[0x77; 32])
    }

    fn writer(dir: &std::path::Path) -> Writer {
        let mut w = Writer::new(Store::at(&dir.join("streams.db")), signing());
        w.add_key(0, Zeroizing::new(MK));
        w
    }

    fn step(text: &str, cursor: &str) -> Step {
        Step {
            fmt: record::FMT_FEED_V1,
            body: text.as_bytes().to_vec(),
            cursor: cursor.into(),
            diff: None,
        }
    }

    /// Send `frames`, feed the answers back, send the head after each commit.
    fn exchange(w: &mut Writer, hub: &mut FakeHub, frames: Vec<Frame>) -> Vec<Answer> {
        let mut answers = Vec::new();
        for frame in frames {
            if let Some(answer) = hub.take(frame) {
                let a = match answer {
                    Frame::Ack { ch, head, .. } => w.on_ack(ch, head, 1),
                    Frame::Conflict { ch, head, .. } => w.on_conflict(ch, head),
                    _ => Answer::Unknown,
                };
                if let Answer::Committed { stream, .. } = &a {
                    let row = w.store.get(stream).unwrap();
                    hub.take(w.head(&row, 10).unwrap());
                }
                answers.push(a);
            }
        }
        answers
    }

    #[test]
    fn appends_are_written_ahead_committed_on_ack_and_survive_a_lost_ack_and_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let stream = record::stream_id(&MK, "claude", "s1").unwrap();
        let mut hub = FakeHub::default();
        let mut w = writer(dir.path());
        let row = w.store.create(&stream, Kind::Session, "claude", "s1", 0, None, 1);
        let f = w.append(&row, &[step("one", "c1"), step("two", "c2")]).unwrap().unwrap();
        assert_eq!(exchange(&mut w, &mut hub, vec![f]), vec![Answer::Committed { stream, checkpoint: false }]);
        let row = w.store.get(&stream).unwrap();
        assert_eq!((row.next_seq, row.cursor.as_str(), row.pending.is_none()), (2, "c2", true));

        // The hub commits the next append and the answer is lost; the agent "crashes".
        let f = w.append(&row, &[step("three", "c3")]).unwrap().unwrap();
        hub.lose_next_answer = true;
        assert!(exchange(&mut w, &mut hub, vec![f]).is_empty());
        drop(w);
        let mut w = writer(dir.path());
        let row = w.store.get(&stream).unwrap();
        assert_eq!((row.next_seq, row.cursor.as_str()), (2, "c2"), "nothing moved without the ACK");
        // Re-sent byte for byte: the hub acknowledges what it already holds, and nothing is doubled.
        let again = w.resend(&row).unwrap();
        assert_eq!(
            exchange(&mut w, &mut hub, vec![again]),
            vec![Answer::Committed { stream, checkpoint: false }]
        );
        let row = w.store.get(&stream).unwrap();
        assert_eq!((row.next_seq, row.cursor.as_str()), (3, "c3"));
        assert_eq!(hub.read(stream), vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]);
    }

    #[test]
    fn a_cas_conflict_is_answered_with_a_rebase_readers_restart_from() {
        let dir = tempfile::tempdir().unwrap();
        let stream = record::stream_id(&MK, "claude", "s2").unwrap();
        let mut hub = FakeHub::default();
        let mut w = writer(dir.path());
        let row = w.store.create(&stream, Kind::Session, "claude", "s2", 0, None, 1);
        let f = w.append(&row, &[step("one", "c1")]).unwrap().unwrap();
        exchange(&mut w, &mut hub, vec![f]);
        // The hub moved on without us (another writer with the stream key, or our lost state).
        hub.next.insert(stream, 7);
        let row = w.store.get(&stream).unwrap();
        let f = w.append(&row, &[step("two", "c2")]).unwrap().unwrap();
        let answers = exchange(&mut w, &mut hub, vec![f]);
        assert_eq!(answers, vec![Answer::Conflict { stream, head: 7 }]);
        let row = w.store.get(&stream).unwrap();
        assert!(row.pending.is_none(), "the refused append is dropped");
        // The rebase: the whole state as a page, at the hub's head, with a fresh link.
        let plan = Plan {
            fmt: record::FMT_FEED_V1,
            pages: vec![PageSpec::New {
                body: b"state: one two".to_vec(),
                from: 0,
                to: 2,
                fp: "fp2".into(),
                last_ms: None,
                window: false,
            }],
            upto: 7.max(row.next_seq),
            fresh_link: true,
            cursor: "c2".into(),
            state_bytes: 14,
            rows: None,
        };
        let frames = w.checkpoint(&row, plan, 5).unwrap();
        assert_eq!(exchange(&mut w, &mut hub, frames), vec![Answer::Committed { stream, checkpoint: true }]);
        let row = w.store.get(&stream).unwrap();
        assert_eq!(
            (row.next_seq, row.cursor.as_str(), row.manifest_upto, row.pages.len()),
            (7, "c2", Some(7), 1)
        );
        assert_ne!(row.link, [0; 32]);
        // And appends carry on from the rebase, readable from the start.
        let f = w.append(&row, &[step("three", "c3")]).unwrap().unwrap();
        exchange(&mut w, &mut hub, vec![f]);
        assert_eq!(hub.read(stream), vec![b"state: one two".to_vec(), b"three".to_vec()]);
    }

    /// Host identity renewal (`docs/host-identity-rotation.md` §5.4): readers
    /// take a manifest from the hub under the machine's current key only, so
    /// after a renewal a stream nothing rewrites (outside the sync window)
    /// gets its manifest signed again — the same position, link and pages,
    /// no page sent again — and reads as before under the new key.
    #[test]
    fn a_manifest_is_signed_again_with_a_renewed_key_without_its_pages() {
        let dir = tempfile::tempdir().unwrap();
        let stream = record::stream_id(&MK, "claude", "old").unwrap();
        let mut hub = FakeHub::default();
        let mut w = writer(dir.path());
        let row = w.store.create(&stream, Kind::Session, "claude", "old", 0, None, 1);
        let f = w.append(&row, &[step("one", "c1"), step("two", "c2")]).unwrap().unwrap();
        exchange(&mut w, &mut hub, vec![f]);
        let row = w.store.get(&stream).unwrap();
        let plan = Plan {
            fmt: record::FMT_FEED_V1,
            pages: vec![PageSpec::body(b"state: one two".to_vec())],
            upto: row.next_seq,
            fresh_link: false,
            cursor: "c2".into(),
            state_bytes: 14,
            rows: None,
        };
        let frames = w.checkpoint(&row, plan, 5).unwrap();
        exchange(&mut w, &mut hub, frames);
        let before = w.store.get(&stream).unwrap();
        assert_eq!(before.manifest_upto, Some(before.next_seq));
        let pages = hub.pages.len();
        drop(w);
        // Renewed: the writer starts again with the new key.
        let renewed = SigningKey::from_bytes(&[0x78; 32]);
        hub.renewed = Some(renewed.verifying_key().to_bytes());
        assert!(!hub.manifest[&stream].verify(&renewed.verifying_key().to_bytes()), "the old key signed it");
        let mut w = Writer::new(Store::at(&dir.path().join("streams.db")), renewed.clone());
        w.add_key(0, Zeroizing::new(MK));
        let frame = w.resign(&before, 20).expect("a manifest to sign again");
        assert!(matches!(&frame, Frame::Checkpoint { upto_seq, .. } if *upto_seq == before.next_seq));
        assert!(w.resign(&before, 20).is_none(), "one request in flight");
        assert_eq!(exchange(&mut w, &mut hub, vec![frame]), vec![Answer::Resigned { stream }]);
        assert_eq!(hub.pages.len(), pages, "no page sent again");
        let m = &hub.manifest[&stream];
        assert!(m.verify(&renewed.verifying_key().to_bytes()));
        assert_eq!(m.pages, before.pages.iter().map(|p| crate::agent::hub::unhex32(&p.hash).unwrap()).collect::<Vec<_>>());
        let after = w.store.get(&stream).unwrap();
        assert_eq!(
            (after.next_seq, after.link, after.manifest_upto, after.pages.len(), after.updated_ms),
            (before.next_seq, before.link, before.manifest_upto, before.pages.len(), before.updated_ms),
            "nothing of the stream moved"
        );
        assert_eq!(hub.read(stream), vec![b"state: one two".to_vec()], "readable under the new key");
        // Records past the manifest: no re-signing (a checkpoint does it).
        let f = w.append(&after, &[step("three", "c3")]).unwrap().unwrap();
        exchange(&mut w, &mut hub, vec![f]);
        assert!(w.resign(&w.store.get(&stream).unwrap(), 30).is_none());
    }

    #[test]
    fn a_compaction_keeps_the_link_and_reuses_unchanged_pages() {
        let dir = tempfile::tempdir().unwrap();
        let stream = record::stream_id(&MK, "codex", "s3").unwrap();
        let mut hub = FakeHub::default();
        let mut w = writer(dir.path());
        let row = w.store.create(&stream, Kind::Session, "codex", "s3", 0, None, 1);
        let f = w.append(&row, &[step("a", "1"), step("b", "2")]).unwrap().unwrap();
        exchange(&mut w, &mut hub, vec![f]);
        let row = w.store.get(&stream).unwrap();
        let link_before = row.link;
        let plan = Plan {
            fmt: record::FMT_FEED_V1,
            pages: vec![PageSpec::New {
                body: b"a b".to_vec(),
                from: 0,
                to: 2,
                fp: "f2".into(),
                last_ms: None,
                window: false,
            }],
            upto: row.next_seq,
            fresh_link: false,
            cursor: row.cursor.clone(),
            state_bytes: 3,
            rows: None,
        };
        let frames = w.checkpoint(&row, plan, 5).unwrap();
        exchange(&mut w, &mut hub, frames);
        let row = w.store.get(&stream).unwrap();
        assert_eq!((row.next_seq, row.link, row.tail_records), (2, link_before, 0));
        let f = w.append(&row, &[step("c", "3")]).unwrap().unwrap();
        exchange(&mut w, &mut hub, vec![f]);
        let row = w.store.get(&stream).unwrap();
        // The next compaction reuses the first page and adds one.
        let plan = Plan {
            fmt: record::FMT_FEED_V1,
            pages: vec![
                PageSpec::Reuse(row.pages[0].clone()),
                PageSpec::New {
                    body: b"c".to_vec(),
                    from: 2,
                    to: 3,
                    fp: "f3".into(),
                    last_ms: None,
                    window: false,
                },
            ],
            upto: row.next_seq,
            fresh_link: false,
            cursor: row.cursor.clone(),
            state_bytes: 4,
            rows: None,
        };
        let frames = w.checkpoint(&row, plan, 6).unwrap();
        assert_eq!(
            frames.iter().filter(|f| matches!(f, Frame::Page { .. })).count(),
            1,
            "only the new page is uploaded"
        );
        exchange(&mut w, &mut hub, frames);
        assert_eq!(hub.read(stream), vec![b"a b".to_vec(), b"c".to_vec()]);
    }

    #[test]
    fn only_one_request_per_stream_is_in_flight_and_a_disconnect_forgets_them() {
        let dir = tempfile::tempdir().unwrap();
        let stream = record::stream_id(&MK, "claude", "s4").unwrap();
        let mut w = writer(dir.path());
        let row = w.store.create(&stream, Kind::Session, "claude", "s4", 0, None, 1);
        assert!(w.append(&row, &[step("x", "1")]).unwrap().is_some());
        let row = w.store.get(&stream).unwrap();
        assert!(w.append(&row, &[step("y", "2")]).unwrap().is_none(), "busy, and pending");
        assert!(w.resend(&row).is_none(), "already in flight");
        w.disconnected();
        assert!(w.resend(&row).is_some(), "after a reconnect, the pending append goes again");
        // Many large bodies: one APPEND takes what fits a frame.
        let dir2 = tempfile::tempdir().unwrap();
        let mut w2 = writer(dir2.path());
        let s2 = record::stream_id(&MK, "claude", "s5").unwrap();
        let row2 = w2.store.create(&s2, Kind::Session, "claude", "s5", 0, None, 1);
        // 15 KB bodies pad to 16 KiB records: three fit a frame, four do not.
        let big: Vec<Step> = (0..5).map(|i| step(&"z".repeat(15_000), &i.to_string())).collect();
        let Some(Frame::Append { records, .. }) = w2.append(&row2, &big).unwrap() else {
            panic!()
        };
        assert_eq!(records.len(), 3);
        assert_eq!(
            w2.store.get(&s2).unwrap().pending.unwrap().cursor,
            "2",
            "the cursor of the last body sent"
        );
    }
}
