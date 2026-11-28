//! Reading a transcript on from where an earlier read of it stopped.
//!
//! A transcript is only ever appended to, so a parse of it can go on from
//! the end of the last one rather than read it all again ([`ResumableLines`]).
//! What makes that safe is proving the file now is the one read then, grown:
//! the [`ReadPoint`] a read leaves says where it stopped and fingerprints the
//! file's first line and the bytes just before the stop. A file shorter than
//! that, or whose first line or last bytes read differ, is another file, or
//! this one rewritten: it is read from the start again. What can still slip
//! through is a rewrite that keeps both ends of the old text and changes its
//! middle, which no CLI does; the caller adds a cheap check of its own (that
//! the file's signature only grew) and parses from the start now and then.
//!
//! A line still being written is not read until it ends. The piece after the
//! last newline is read only when it is a whole entry already, as a reader of
//! the whole file would read it; the next read then goes on only once its
//! newline follows it, since anything else would have made it part of a
//! longer line.
//!
//! A compressed transcript ([`crate::compressed`]) is read from the start like
//! any other, and its read point is in its text, which is the plain file's: a
//! read of a rollout Codex compressed goes on in the plain file Codex writes
//! back before it appends. A compressed one cannot be read on from a point,
//! and it does not grow.

use std::any::Any;
use std::io;
use std::path::Path;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncSeekExt};

use crate::compressed;
use crate::util::{line_text, try_parse, TranscriptItem};
use droppen_instance::paths::Paths;

/// The most of the file's first line, and of its last bytes read, that a
/// [`ReadPoint`] fingerprints.
const MARK_MAX: usize = 4096;

/// The most a parse reads on from an earlier one ([`ResumableLines::reopen`]).
/// More appended than this, and the file is read from its start: a parse of
/// 32 MiB is a few dozen milliseconds, and the bytes are held whole meanwhile.
pub const READ_ON_MAX: u64 = 32 * 1024 * 1024;

/// A reader whose parse can go on from where an earlier one stopped
/// ([`crate::Transcripts::resumable`]).
///
/// Its two parses have to agree: whatever parse it went on from, the items
/// [`Resumable::resume`] makes are the ones [`Resumable::read`] makes of the
/// whole file. The reader's own tests prove that against a parse from the
/// start, and the loader can check it as it runs (`DROPPEN_PARSE_ORACLE`).
pub trait Resumable: Send + Sync {
    /// A parse of the transcript at `file_path` from its start, reading
    /// `lines` ([`ResumableLines::open`]).
    fn read<'a>(&'a self, paths: &'a Paths, file_path: &'a Path, lines: ResumableLines) -> BoxFuture<'a, io::Result<Resumed>>;

    /// The parse an earlier one, of `items`, which left `state`, makes once
    /// it reads `lines` too ([`ResumableLines::reopen`]); `None` when it
    /// cannot go on from it: `state` is not this reader's, `items` are not
    /// the ones it made, or something besides the transcript that the parse
    /// read has moved since.
    fn resume<'a>(
        &'a self,
        paths: &'a Paths,
        file_path: &'a Path,
        lines: ResumableLines,
        items: Vec<Arc<TranscriptItem>>,
        state: Box<dyn Any + Send>,
    ) -> BoxFuture<'a, io::Result<Option<Resumed>>>;

    /// A fingerprint of what a parse of `file_path` reads besides the
    /// transcript (a subagent's sidecar, a chat's delivery rows), for the
    /// check of a parse that went on against a whole one: while it holds
    /// still, the two read the same. 0 for a reader that reads nothing else.
    fn inputs<'a>(&'a self, paths: &'a Paths, file_path: &'a Path) -> BoxFuture<'a, u64> {
        let _ = (paths, file_path);
        Box::pin(std::future::ready(0))
    }
}

/// A parse a later one can go on from.
pub struct Resumed {
    /// The items, each shared.
    pub items: Vec<Arc<TranscriptItem>>,
    /// The lowest index whose item may differ from the parse this one went on
    /// from ([`crate::item_log::ItemLog::changed_from`]); 0 from the start.
    pub changed_from: usize,
    /// Where it stopped reading.
    pub point: ReadPoint,
    /// What the reader keeps of it to go on from, which only it can read.
    pub state: Box<dyn Any + Send>,
}

/// A stretch of the file, fingerprinted: `len` bytes and their FNV-1a hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mark {
    len: u64,
    hash: u64,
}

impl Mark {
    /// The mark of `parts`, end to end.
    fn of(parts: &[&[u8]]) -> Self {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let mut len = 0u64;
        for part in parts {
            len += part.len() as u64;
            for byte in *part {
                hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        Self { len, hash }
    }
}

/// Where a read of a transcript stopped, and what the file held there: what
/// [`ResumableLines::reopen`] proves the file against before it reads on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadPoint {
    /// How many bytes of the text were read: the end of the last line taken.
    end: u64,
    /// Whether that line had no newline yet ([`ResumableLines`]).
    open_tail: bool,
    /// The file's first line, at most [`MARK_MAX`] bytes of it. `None` only
    /// when nothing was read.
    head: Option<Mark>,
    /// The last bytes read, at most [`MARK_MAX`] of them, back from `end`.
    last: Option<Mark>,
}

impl ReadPoint {
    /// How many bytes of the text were read.
    pub fn end(&self) -> u64 {
        self.end
    }

    /// Whether the last line read had no newline yet.
    pub fn open_tail(&self) -> bool {
        self.open_tail
    }
}

/// How many lines [`ResumableLines::next_line`] returns before yielding
/// cooperatively to the async executor so parsing a large buffered transcript
/// does not starve Tokio worker threads.
const YIELD_EVERY_LINES: u16 = 256;

/// A transcript's lines, decoded lossily as [`crate::util::LossyLines`] does,
/// from its start or from where an earlier read stopped, keeping count of
/// where they end.
pub struct ResumableLines {
    reader: Box<dyn AsyncBufRead + Send + Unpin>,
    /// The line being read.
    buffer: Vec<u8>,
    /// The last two lines taken, as bytes, the newer last: what the read
    /// point's mark of the last bytes is made of, kept by swapping buffers
    /// rather than copying lines.
    older: Vec<u8>,
    newer: Vec<u8>,
    /// How many bytes of the text have been taken.
    end: u64,
    open_tail: bool,
    head: Option<Mark>,
    lines_since_yield: u16,
}

impl ResumableLines {
    /// The transcript `path` names, from its start, in the form it is on disk.
    pub async fn open(path: &Path) -> io::Result<Self> {
        Ok(Self::at(compressed::open_text(path, compressed::WHOLE_READ).await?, 0, false, None, Vec::new()))
    }

    /// The text of the transcript `path` names, from its start up to `end`:
    /// what a read that stopped there read, and nothing appended since.
    pub async fn open_upto(path: &Path, end: u64) -> io::Result<Self> {
        let text = compressed::open_text(path, compressed::WHOLE_READ).await?;
        Ok(Self::at(Box::new(text.take(end)), 0, false, None, Vec::new()))
    }

    /// The transcript `path` names, from where the read that left `point`
    /// stopped, when it is that read's file grown; `None` when it cannot be
    /// shown to be, or is compressed, and has to be read from its start, or
    /// when more than `limit` bytes were appended since, which reading from
    /// the start costs about as little as reading on.
    ///
    /// What was appended is read here, whole, so an error reading the file
    /// is this call's, and the lines it hands out come from memory: a parse
    /// that goes on from an earlier one either has every line or has read
    /// none, and is never left halfway through them.
    pub async fn reopen(path: &Path, point: &ReadPoint, limit: u64) -> io::Result<Option<Self>> {
        let (found, mut file) = compressed::open(path).await?;
        if compressed::is_compressed(&found) {
            return Ok(None);
        }
        let len = file.metadata().await?.len();
        if len < point.end || len - point.end > limit {
            return Ok(None);
        }
        let mut last = Vec::new();
        if point.end > 0 {
            let (Some(head), Some(mark)) = (point.head, point.last) else {
                return Ok(None);
            };
            let Some(first) = read_at(&mut file, 0, head.len).await? else {
                return Ok(None);
            };
            let Some(bytes) = read_at(&mut file, point.end - mark.len, mark.len).await? else {
                return Ok(None);
            };
            if Mark::of(&[&first]) != head || Mark::of(&[&bytes]) != mark {
                return Ok(None);
            }
            last = bytes;
        }
        // A last line taken without its newline was whole only if its newline
        // is what came after it, carriage return or not. A carriage return
        // alone cannot say yet.
        if point.open_tail {
            let after = len - point.end;
            if after > 0 {
                let Some(next) = read_at(&mut file, point.end, after.min(2)).await? else {
                    return Ok(None);
                };
                if !(next.starts_with(b"\n") || next == b"\r\n") {
                    return Ok(None);
                }
            }
        }
        // Up to the limit, and a byte past it to tell a file that grew again
        // meanwhile; what came since the length was looked at is read too.
        // Sized for what the length says, with room for the read that finds
        // the end, so the buffer is not grown by doubling to twice that.
        file.seek(io::SeekFrom::Start(point.end)).await?;
        let mut appended = Vec::with_capacity(usize::try_from(len - point.end).unwrap_or(0).saturating_add(64));
        file.take(limit.saturating_add(1)).read_to_end(&mut appended).await?;
        if appended.len() as u64 > limit {
            return Ok(None);
        }
        let reader = Box::new(io::Cursor::new(appended));
        Ok(Some(Self::at(reader, point.end, point.open_tail, point.head, last)))
    }

    fn at(reader: Box<dyn AsyncBufRead + Send + Unpin>, end: u64, open_tail: bool, head: Option<Mark>, last: Vec<u8>) -> Self {
        Self {
            reader,
            buffer: Vec::new(),
            older: Vec::new(),
            newer: last,
            end,
            open_tail,
            head,
            lines_since_yield: 0,
        }
    }

    /// The next line without its terminator, or `None` at the end of what
    /// can be read yet: the end of the file, or a last line still being
    /// written.
    pub async fn next_line(&mut self) -> io::Result<Option<String>> {
        self.lines_since_yield = self.lines_since_yield.wrapping_add(1);
        if self.lines_since_yield >= YIELD_EVERY_LINES {
            self.lines_since_yield = 0;
            tokio::task::yield_now().await;
        }
        self.buffer.clear();
        let read = self.reader.read_until(b'\n', &mut self.buffer).await?;
        if read == 0 {
            return Ok(None);
        }
        let ended = self.buffer.last() == Some(&b'\n');
        let text = line_text(&self.buffer);
        // No newline after it: the end of the file. Taken only when it is a
        // whole entry, which a reader of the whole file would parse; anything
        // else it would skip, and it is read again once it is a line.
        if !ended && try_parse(&text).is_none() {
            return Ok(None);
        }
        if self.end == 0 && self.head.is_none() {
            self.head = Some(Mark::of(&[&self.buffer[..self.buffer.len().min(MARK_MAX)]]));
        }
        self.end += read as u64;
        self.open_tail = !ended;
        std::mem::swap(&mut self.older, &mut self.newer);
        std::mem::swap(&mut self.newer, &mut self.buffer);
        Ok(Some(text))
    }

    /// Where the lines taken so far end, for a later read to go on from.
    pub fn point(&self) -> ReadPoint {
        let newer = &self.newer[self.newer.len().saturating_sub(MARK_MAX)..];
        let older = &self.older[self.older.len().saturating_sub(MARK_MAX - newer.len())..];
        ReadPoint {
            end: self.end,
            open_tail: self.open_tail,
            head: self.head,
            last: (self.end > 0).then(|| Mark::of(&[older, newer])),
        }
    }
}

/// `len` bytes of `file` from `at`, or `None` when it ends before them.
async fn read_at(file: &mut tokio::fs::File, at: u64, len: u64) -> io::Result<Option<Vec<u8>>> {
    file.seek(io::SeekFrom::Start(at)).await?;
    let mut bytes = vec![0; usize::try_from(len).map_err(io::Error::other)?];
    match file.read_exact(&mut bytes).await {
        Ok(_) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    async fn all(lines: &mut ResumableLines) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(line) = lines.next_line().await.expect("read") {
            out.push(line);
        }
        out
    }

    fn append(path: &Path, bytes: &[u8]) {
        std::fs::OpenOptions::new().append(true).create(true).open(path).expect("open").write_all(bytes).expect("append");
    }

    /// A read goes on where the last one stopped, and only there.
    #[tokio::test]
    async fn a_grown_file_is_read_on_from_where_the_last_read_stopped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.jsonl");
        append(&path, b"{\"n\":1}\r\n{\"n\":2}\n");
        let mut lines = ResumableLines::open(&path).await.expect("open");
        assert_eq!(all(&mut lines).await, ["{\"n\":1}", "{\"n\":2}"], "the carriage return is not the JSON's");
        let point = lines.point();
        assert_eq!((point.end(), point.open_tail()), (17, false));

        append(&path, b"{\"n\":3}\n");
        let mut more = ResumableLines::reopen(&path, &point, READ_ON_MAX).await.expect("reopen").expect("the same file, grown");
        assert_eq!(all(&mut more).await, ["{\"n\":3}"]);
        assert_eq!(more.point().end(), 25);

        let mut nothing = ResumableLines::reopen(&path, &more.point(), READ_ON_MAX).await.expect("reopen").expect("unchanged");
        assert!(all(&mut nothing).await.is_empty());
        assert_eq!(nothing.point(), more.point(), "a read of nothing new leaves the same point");
    }

    /// A line still being written is left for later; one that is a whole
    /// entry already is taken, and the next read goes on only past a newline.
    #[tokio::test]
    async fn the_last_line_is_taken_only_when_it_is_whole() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.jsonl");
        append(&path, b"{\"n\":1}\n{\"n\":");
        let mut lines = ResumableLines::open(&path).await.expect("open");
        assert_eq!(all(&mut lines).await, ["{\"n\":1}"]);
        let point = lines.point();
        assert_eq!((point.end(), point.open_tail()), (8, false), "the half line is not read");

        append(&path, b"2}");
        let mut whole = ResumableLines::reopen(&path, &point, READ_ON_MAX).await.expect("reopen").expect("grown");
        assert_eq!(all(&mut whole).await, ["{\"n\":2}"], "a whole entry, newline or not");
        let open = whole.point();
        assert_eq!((open.end(), open.open_tail()), (15, true));

        append(&path, b"\n{\"n\":3}\n");
        let mut on = ResumableLines::reopen(&path, &open, READ_ON_MAX).await.expect("reopen").expect("its newline came");
        assert_eq!(all(&mut on).await, ["", "{\"n\":3}"], "the newline ends the line already read");

        std::fs::write(&path, b"{\"n\":1}\n{\"n\":2}{\"n\":9}\n").expect("rewrite");
        assert!(
            ResumableLines::reopen(&path, &open, READ_ON_MAX).await.expect("reopen").is_none(),
            "the line went on past what was read as a whole entry"
        );
    }

    /// A file that is not the one read, grown, is read from its start: a
    /// shorter one, another first line, other bytes before the stop, or a
    /// compressed one.
    #[tokio::test]
    async fn a_file_that_is_not_the_one_read_grown_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.jsonl");
        let text = b"{\"first\":1}\n{\"n\":2}\n{\"n\":3}\n";
        std::fs::write(&path, text).expect("write");
        let mut lines = ResumableLines::open(&path).await.expect("open");
        all(&mut lines).await;
        let point = lines.point();
        let refused = |bytes: &'static [u8]| {
            let path = path.clone();
            let point = point.clone();
            async move {
                std::fs::write(&path, bytes).expect("write");
                ResumableLines::reopen(&path, &point, READ_ON_MAX).await.expect("reopen").is_none()
            }
        };
        assert!(refused(b"{\"first\":1}\n{\"n\":2}\n").await, "shorter");
        assert!(refused(b"{\"other\":1}\n{\"n\":2}\n{\"n\":3}\n{\"n\":4}\n").await, "another first line");
        assert!(refused(b"{\"first\":1}\n{\"n\":2}\n{\"n\":4}\n{\"n\":5}\n").await, "other bytes before the stop");
        assert!(!refused(b"{\"first\":1}\n{\"n\":2}\n{\"n\":3}\n{\"n\":4}\n").await, "the same, grown");

        std::fs::remove_file(&path).expect("remove");
        std::fs::write(compressed::compressed_path(&path), compressed::compress(text)).expect("compress");
        assert!(ResumableLines::reopen(&path, &point, READ_ON_MAX).await.expect("reopen").is_none(), "compressed");
        let mut decoded = ResumableLines::open(&path).await.expect("open");
        all(&mut decoded).await;
        assert_eq!(decoded.point(), point, "the text read compressed ends where the plain one did");
    }

    /// More appended than the limit is read from the start instead; and a
    /// read up to a point reads what the read that stopped there did.
    #[tokio::test]
    async fn a_read_on_stops_at_its_limit_and_a_read_up_to_a_point_at_the_point() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.jsonl");
        append(&path, b"{\"n\":1}\n");
        let mut lines = ResumableLines::open(&path).await.expect("open");
        all(&mut lines).await;
        let point = lines.point();
        append(&path, b"{\"n\":2}\n{\"n\":3}\n");
        assert!(ResumableLines::reopen(&path, &point, 15).await.expect("reopen").is_none(), "16 bytes appended");
        let mut on = ResumableLines::reopen(&path, &point, 16).await.expect("reopen").expect("within the limit");
        assert_eq!(all(&mut on).await, ["{\"n\":2}", "{\"n\":3}"]);

        let mut upto = ResumableLines::open_upto(&path, point.end()).await.expect("open");
        assert_eq!(all(&mut upto).await, ["{\"n\":1}"]);
        assert_eq!(upto.point(), point);
    }
}
