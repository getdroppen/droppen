//! Transcripts a CLI has compressed where they lie.
//!
//! Codex compresses a rollout nobody has written to for a week: the file
//! becomes `<name>.jsonl.zst` beside it, one zstd frame, and the plain
//! `<name>.jsonl` is deleted. Its state database goes on naming the plain
//! file, and Codex decompresses the rollout back to that name before it next
//! appends (`codex-rs/rollout/src/compression.rs`). So either form can be the
//! one on disk, and either can replace the other at any moment.
//!
//! [`existing`] finds the form that is there, by Codex's own rule. The readers
//! in [`crate::util`] open either form, and follow a transcript that changed
//! form between being found and being opened ([`open`]). A compressed one is
//! read through a pure-Rust decoder ([`ruzstd`]): nothing in the server links
//! a C library.
//!
//! A compressed transcript is cold by construction, so it is read whole, from
//! the start, every time it is read at all: a zstd frame cannot be read
//! backwards or from the middle, and nothing has to, since what is read of
//! one is cached against a file that does not change.

use std::ffi::OsStr;
use std::io::{self, BufRead, Read};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{LazyLock, Mutex};
use std::task::{ready, Context, Poll};

use ruzstd::decoding::errors::{FrameDecoderError, ReadFrameHeaderError};
use ruzstd::decoding::{BlockDecodingStrategy, FrameDecoder};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncReadExt, ReadBuf};

use crate::util::FileCache;

/// What a compressed transcript's name ends in.
const COMPRESSED_SUFFIX: &str = ".jsonl.zst";

/// How much decoded text one message from the decoding thread carries.
const CHUNK: usize = 64 * 1024;

/// How many decoded chunks may wait for a slow reader before the decoder
/// waits for it: what bounds a streamed transcript's memory.
const CHUNKS_IN_FLIGHT: usize = 4;

/// The longest a zstd frame header is: the magic number and at most 14 bytes.
const FRAME_HEADER_MAX: u64 = 18;

/// `path`'s file name, when it has one that is UTF-8 — as Codex reads one.
fn file_name(path: &Path) -> Option<&str> {
    path.file_name().and_then(OsStr::to_str)
}

/// Whether `path` names a compressed transcript (`*.jsonl.zst`).
pub fn is_compressed(path: &Path) -> bool {
    file_name(path).is_some_and(|name| name.ends_with(COMPRESSED_SUFFIX))
}

/// The plain form of a transcript path: `path` without its `.zst`.
pub fn plain_path(path: &Path) -> PathBuf {
    match file_name(path).filter(|name| name.ends_with(COMPRESSED_SUFFIX)) {
        Some(name) => path.with_file_name(&name[..name.len() - ".zst".len()]),
        None => path.to_path_buf(),
    }
}

/// The compressed form of a transcript path: `path` with `.zst` after its name.
pub fn compressed_path(path: &Path) -> PathBuf {
    match path.file_name() {
        Some(name) if !is_compressed(path) => {
            let mut name = name.to_os_string();
            name.push(".zst");
            path.with_file_name(name)
        }
        _ => path.to_path_buf(),
    }
}

/// The form a transcript at `path` turns into when it is compressed or
/// decompressed, or `None` for a file that is not a JSONL transcript.
fn other_form(path: &Path) -> Option<PathBuf> {
    if is_compressed(path) {
        return Some(plain_path(path));
    }
    file_name(path)
        .is_some_and(|name| name.ends_with(".jsonl"))
        .then(|| compressed_path(path))
}

/// The file the transcript path `path` names, and its metadata: the plain file
/// when it is there, otherwise its compressed form, otherwise `None`.
///
/// Codex's own rule (`existing_rollout_with_metadata_sync`), whichever form
/// `path` names. Both are on disk only while one is replacing the other —
/// published, the old one not deleted yet — and Codex reads the plain one.
pub async fn existing(path: &Path) -> Option<(PathBuf, std::fs::Metadata)> {
    path.file_name()?;
    let plain = plain_path(path);
    let compressed = compressed_path(&plain);
    for candidate in [plain, compressed] {
        if let Ok(metadata) = tokio::fs::metadata(&candidate).await {
            if metadata.is_file() {
                return Some((candidate, metadata));
            }
        }
    }
    None
}

/// The content lengths read off compressed transcripts' frame headers, by the
/// mtime and size of the file they were read from.
static CONTENT_LENS: LazyLock<FileCache<u64>> = LazyLock::new(|| FileCache::new(4096));

/// How long the transcript at `path` is, as text: its length on disk, or for a
/// compressed one the length its frame header says it decompresses to.
///
/// Codex states that length (`set_pledged_src_size`), so it costs one read of
/// a header, once per file. A header that does not state it, or cannot be
/// read, leaves the length on disk: smaller than the text, never a failure.
pub async fn content_len(path: &Path, metadata: &std::fs::Metadata) -> u64 {
    if !is_compressed(path) {
        return metadata.len();
    }
    let mtime = droppen_base::time::mtime_ms(metadata);
    if let Some(len) = CONTENT_LENS.get(path, mtime, metadata.len()) {
        return len;
    }
    let len = frame_content_len(path).await.unwrap_or(metadata.len());
    CONTENT_LENS.put(path, mtime, metadata.len(), len);
    len
}

/// The decompressed length the first frame header of `path` states, if it does.
async fn frame_content_len(path: &Path) -> Option<u64> {
    let mut header = Vec::new();
    tokio::fs::File::open(path)
        .await
        .ok()?
        .take(FRAME_HEADER_MAX)
        .read_to_end(&mut header)
        .await
        .ok()?;
    let mut frame = FrameDecoder::new();
    frame.init(header.as_slice()).ok()?;
    // Zero is how the decoder says the header named no length.
    Some(frame.content_size()).filter(|len| *len > 0)
}

/// A transcript opened in whichever form it was on disk.
pub(crate) enum Opened {
    /// The plain JSONL file.
    Plain(tokio::fs::File),
    /// The compressed file, to be read through [`Frames`] on a blocking thread.
    Compressed(CompressedFile),
}

/// A compressed transcript, open.
pub(crate) struct CompressedFile {
    file: std::fs::File,
    path: PathBuf,
    /// Its mtime and length on disk as it was opened: what a [`Tail`] of it is
    /// kept against.
    mark: (f64, u64),
}

/// Open the transcript at `path`.
///
/// One that was compressed or decompressed between being found and being
/// opened is gone from `path` and is in its other form: that is opened
/// instead, once. Anything else is the error opening `path` gave.
pub(crate) async fn open(path: &Path) -> io::Result<Opened> {
    match open_as_named(path).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => match other_form(path) {
            Some(other) => open_as_named(&other).await.map_err(|_| error),
            None => Err(error),
        },
        opened => opened,
    }
}

async fn open_as_named(path: &Path) -> io::Result<Opened> {
    let file = tokio::fs::File::open(path).await?;
    if !is_compressed(path) {
        return Ok(Opened::Plain(file));
    }
    let metadata = file.metadata().await?;
    Ok(Opened::Compressed(CompressedFile {
        file: file.into_std().await,
        path: path.to_path_buf(),
        mark: (droppen_base::time::mtime_ms(&metadata), metadata.len()),
    }))
}

/// The text of the transcript at `path`, as a stream: a plain file as it is,
/// a compressed one decoded on a blocking thread a chunk at a time, so a long
/// rollout is never held whole. Opened as [`open`] opens it.
pub(crate) async fn open_text(path: &Path) -> io::Result<Box<dyn AsyncBufRead + Send + Unpin>> {
    Ok(match open(path).await? {
        Opened::Plain(file) => Box::new(tokio::io::BufReader::new(file)),
        Opened::Compressed(compressed) => Box::new(Decoded::spawn(compressed.file)),
    })
}

impl CompressedFile {
    /// Run `read` over the decoded text, on a blocking thread: decoding is CPU
    /// work and the file is read with blocking calls.
    pub(crate) async fn with_text<T: Send + 'static>(self, read: impl FnOnce(&mut dyn Read) -> io::Result<T> + Send + 'static) -> io::Result<T> {
        let file = self.file;
        tokio::task::spawn_blocking(move || read(&mut Frames::new(io::BufReader::new(file))))
            .await
            .map_err(io::Error::other)?
    }

    /// The last `max_bytes` of the text, and whether anything before them was
    /// left out.
    ///
    /// The whole text is decoded to reach its end, so the end is kept
    /// ([`TAILS`]): a session view asks its transcript's tail for the model
    /// and the context every two seconds, and the list asks each subagent
    /// thread's on every pass, of a file that does not change.
    pub(crate) async fn tail(self, max_bytes: u64) -> io::Result<(Vec<u8>, bool)> {
        if let Some(tail) = Tail::kept(&self.path, self.mark, max_bytes) {
            return Ok(tail);
        }
        let (path, mark) = (self.path.clone(), self.mark);
        let (bytes, cut) = self.with_text(move |text| decoded_tail(text, max_bytes)).await?;
        let tail = (bytes.clone(), cut);
        Tail::keep(Tail {
            path,
            mark,
            kept: max_bytes,
            bytes,
            cut,
        });
        Ok(tail)
    }
}

/// The last `max_bytes` of a decoded text, and whether anything before them
/// was left out.
fn decoded_tail(text: &mut dyn Read, max_bytes: u64) -> io::Result<(Vec<u8>, bool)> {
    let keep = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    let mut tail = Vec::new();
    let mut cut = false;
    let mut chunk = vec![0u8; CHUNK];
    loop {
        let read = match text.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        tail.extend_from_slice(&chunk[..read]);
        // Trimmed once it is twice what is kept, so each byte is moved once
        // or twice rather than once per chunk.
        if tail.len() > keep.saturating_mul(2).max(CHUNK) {
            tail.drain(..tail.len() - keep);
            cut = true;
        }
    }
    if tail.len() > keep {
        tail.drain(..tail.len() - keep);
        cut = true;
    }
    Ok((tail, cut))
}

/// The end of a compressed transcript's text, decoded and kept.
struct Tail {
    path: PathBuf,
    /// The file's mtime and length when it was decoded.
    mark: (f64, u64),
    /// How much of the end was asked for.
    kept: u64,
    bytes: Vec<u8>,
    /// Whether text before `bytes` was left out.
    cut: bool,
}

/// How many bytes of decoded tails [`TAILS`] holds in all: room for the
/// session view's 1 MiB and a subagent tree's worth of 64 KiB tails.
const TAILS_BUDGET: usize = 16 * 1024 * 1024;

/// The tails decoded last, most recently used last.
static TAILS: LazyLock<Mutex<Vec<Tail>>> = LazyLock::new(|| Mutex::new(Vec::new()));

impl Tail {
    /// The last `max_bytes` of the file at `path` as it was at `mark`, when a
    /// tail kept of it reaches back that far.
    fn kept(path: &Path, mark: (f64, u64), max_bytes: u64) -> Option<(Vec<u8>, bool)> {
        let mut tails = TAILS.lock().expect("tail cache poisoned");
        let at = tails
            .iter()
            .position(|tail| tail.path == path && tail.mark == mark && (tail.kept >= max_bytes || !tail.cut))?;
        let tail = tails.remove(at);
        let skip = tail.bytes.len().saturating_sub(usize::try_from(max_bytes).unwrap_or(usize::MAX));
        let found = (tail.bytes[skip..].to_vec(), tail.cut || skip > 0);
        tails.push(tail);
        Some(found)
    }

    /// Keep `tail`, in place of any other of its file, dropping the least
    /// recently used past [`TAILS_BUDGET`] (and `tail` itself, when it alone
    /// is past it).
    fn keep(tail: Tail) {
        let mut tails = TAILS.lock().expect("tail cache poisoned");
        tails.retain(|kept| kept.path != tail.path);
        tails.push(tail);
        while tails.iter().map(|tail| tail.bytes.len()).sum::<usize>() > TAILS_BUDGET {
            tails.remove(0);
        }
    }
}

/// Every frame of a zstd stream, decoded, as one [`Read`].
///
/// Codex writes one frame. Its own reader (the `zstd` crate's `Decoder`, over
/// libzstd) reads frames written back to back as one text and steps over
/// skippable frames, so this does too. The window is capped where libzstd's
/// decoder caps it by default (128 MiB, which is also [`FrameDecoder`]'s
/// default); Codex compresses at level 3, whose window is at most 2 MiB.
struct Frames<R> {
    source: R,
    frame: FrameDecoder,
    /// Whether a frame's header has been read and its text not all handed out.
    in_frame: bool,
}

impl<R: BufRead> Frames<R> {
    /// The frames of `source`, from its first byte.
    fn new(source: R) -> Self {
        Self {
            source,
            frame: FrameDecoder::new(),
            in_frame: false,
        }
    }
}

fn invalid(error: FrameDecoderError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

impl<R: BufRead> Read for Frames<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.in_frame {
                while self.frame.can_collect() == 0 && !self.frame.is_finished() {
                    self.frame
                        .decode_blocks(&mut self.source, BlockDecodingStrategy::UptoBlocks(1))
                        .map_err(invalid)?;
                }
                let read = self.frame.read(buf)?;
                if read > 0 {
                    return Ok(read);
                }
                // Finished, and all of it handed out.
                self.in_frame = false;
            }
            if self.source.fill_buf()?.is_empty() {
                return Ok(0);
            }
            match self.frame.reset(&mut self.source) {
                Ok(()) => self.in_frame = true,
                Err(FrameDecoderError::ReadFrameHeaderError(ReadFrameHeaderError::SkipFrame { length, .. })) => {
                    let length = u64::from(length);
                    if io::copy(&mut (&mut self.source).take(length), &mut io::sink())? < length {
                        return Err(io::ErrorKind::UnexpectedEof.into());
                    }
                }
                Err(error) => return Err(invalid(error)),
            }
        }
    }
}

/// A compressed transcript's text, decoded on a blocking thread and handed
/// over a chunk at a time.
///
/// Dropping it stops the decoding: the thread's next hand-over finds nobody
/// to take it.
struct Decoded {
    chunks: tokio::sync::mpsc::Receiver<io::Result<Vec<u8>>>,
    chunk: Vec<u8>,
    /// How much of `chunk` has been read.
    at: usize,
}

impl Decoded {
    fn spawn(file: std::fs::File) -> Self {
        let (tx, chunks) = tokio::sync::mpsc::channel(CHUNKS_IN_FLIGHT);
        tokio::task::spawn_blocking(move || {
            let mut frames = Frames::new(io::BufReader::new(file));
            loop {
                let mut chunk = vec![0u8; CHUNK];
                let sent = match frames.read(&mut chunk) {
                    Ok(0) => return,
                    Ok(read) => {
                        chunk.truncate(read);
                        tx.blocking_send(Ok(chunk))
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        let _ = tx.blocking_send(Err(error));
                        return;
                    }
                };
                if sent.is_err() {
                    return;
                }
            }
        });
        Self {
            chunks,
            chunk: Vec::new(),
            at: 0,
        }
    }
}

impl AsyncBufRead for Decoded {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let this = self.get_mut();
        while this.at == this.chunk.len() {
            match ready!(this.chunks.poll_recv(cx)) {
                Some(Ok(chunk)) => {
                    this.chunk = chunk;
                    this.at = 0;
                }
                Some(Err(error)) => return Poll::Ready(Err(error)),
                // The end of the text.
                None => break,
            }
        }
        Poll::Ready(Ok(&this.chunk[this.at..]))
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        let this = self.get_mut();
        this.at = (this.at + amount).min(this.chunk.len());
    }
}

impl AsyncRead for Decoded {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let taken = {
            let available = ready!(self.as_mut().poll_fill_buf(cx))?;
            let taken = available.len().min(buf.remaining());
            buf.put_slice(&available[..taken]);
            taken
        };
        self.consume(taken);
        Poll::Ready(Ok(()))
    }
}

/// `text` compressed as Codex compresses a rollout: one zstd frame, with no
/// checksum, whose header states the length of its text. For the tests of
/// the readers here and above this crate.
///
/// Compressed by the pure-Rust encoder, which leaves the length out of the
/// header; libzstd, told it (`set_pledged_src_size`), puts it in. So it is
/// put in here as libzstd puts it in a frame of more than one segment: eight
/// bytes after the window descriptor, flagged in the header's first byte.
#[cfg(any(test, feature = "test-support"))]
pub fn compress(text: &[u8]) -> Vec<u8> {
    let mut frame = ruzstd::encoding::compress_to_vec(text, ruzstd::encoding::CompressionLevel::Fastest);
    assert_eq!(frame[4], 0, "a header stating no length, single segment, checksum or dictionary");
    frame[4] = 0xC0;
    frame.splice(6..6, (text.len() as u64).to_le_bytes());
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncBufReadExt;

    /// A three-line rollout, as Codex writes it.
    const ROLLOUT: &str = concat!(
        r#"{"timestamp":"2026-05-09T16:39:49.000Z","type":"session_meta","payload":{"id":"x","cwd":"/w"}}"#,
        "\n",
        r#"{"timestamp":"2026-05-09T16:39:50.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"compress me"}]}}"#,
        "\n",
        r#"{"timestamp":"2026-05-09T16:39:51.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}"#,
        "\n",
    );

    /// [`ROLLOUT`] compressed by libzstd 1.5.7 as Codex compresses a rollout:
    /// level 3, the length stated in the header, no checksum
    /// (`zstd -3 --no-check`). One single-segment frame.
    const ROLLOUT_ZST: [u8; 193] = [
        0x28, 0xb5, 0x2f, 0xfd, 0x60, 0xa2, 0x00, 0xbd, 0x05, 0x00, 0x92, 0x0b, 0x24, 0x20, 0x60, 0x87, 0x9b, 0x03, 0x80, 0xc7, 0xde, 0xc4, 0x9a,
        0x95, 0xcd, 0xc9, 0x90, 0xfc, 0x6d, 0x98, 0x0a, 0x02, 0xc0, 0x21, 0xac, 0x78, 0x55, 0x44, 0x9a, 0xb9, 0x70, 0x4a, 0x90, 0x21, 0x88, 0x03,
        0xc8, 0x7b, 0x89, 0xe7, 0xb3, 0x64, 0xb6, 0xc9, 0x65, 0x69, 0x55, 0xa9, 0xa6, 0x42, 0xe2, 0xf9, 0x8b, 0xa0, 0x7a, 0xd7, 0x56, 0x2a, 0x8c,
        0xdb, 0xf8, 0x2e, 0xdb, 0x34, 0xa6, 0x0c, 0xd0, 0xcb, 0xbe, 0x2c, 0xdd, 0xb5, 0x31, 0xfa, 0x8b, 0x74, 0x3e, 0xfe, 0x41, 0x52, 0xfd, 0x5a,
        0x5f, 0xb5, 0x5c, 0x59, 0x36, 0xbd, 0x43, 0x07, 0xe4, 0x3d, 0x33, 0x02, 0x18, 0x0a, 0x1f, 0x37, 0x8c, 0x66, 0x94, 0x1f, 0x68, 0x26, 0xe5,
        0xc2, 0x91, 0x9a, 0x8f, 0xd3, 0x7e, 0x2d, 0x2c, 0x95, 0xea, 0xd5, 0xe8, 0x37, 0xc7, 0xfa, 0xb8, 0x00, 0x21, 0x04, 0xa0, 0x0c, 0xa0, 0x22,
        0x80, 0x88, 0x02, 0x81, 0x82, 0x92, 0x03, 0x4a, 0x10, 0x06, 0x64, 0x30, 0xba, 0xad, 0xb4, 0xfa, 0xa5, 0xd6, 0x8f, 0x0d, 0x00, 0x2b, 0xf3,
        0x86, 0xca, 0x28, 0x77, 0x90, 0x2e, 0x25, 0x43, 0xab, 0x5a, 0x30, 0x34, 0x0b, 0x39, 0xd5, 0x33, 0x0f, 0xcb, 0x04, 0xcb, 0x80, 0x31, 0xc8,
        0x1f, 0x3a, 0x2c, 0x86, 0xae, 0xaa, 0x41, 0x31, 0x03,
    ];

    /// The test-support encoder states the length as libzstd does, so
    /// [`content_len`] reads the same off either.
    #[tokio::test]
    async fn the_test_encoder_writes_frames_as_codex_does() {
        let text = ROLLOUT.repeat(1_000);
        let frame = compress(text.as_bytes());
        assert_eq!(decode(&frame).expect("decode"), text.as_bytes());
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout-1-a.jsonl.zst");
        std::fs::write(&path, &frame).expect("zst");
        let metadata = std::fs::metadata(&path).expect("stat");
        assert_eq!(content_len(&path, &metadata).await, text.len() as u64);
    }

    fn decode(bytes: &[u8]) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        Frames::new(bytes).read_to_end(&mut out)?;
        Ok(out)
    }

    #[test]
    fn a_frame_libzstd_wrote_decodes_to_the_rollout() {
        assert_eq!(decode(&ROLLOUT_ZST).expect("decode"), ROLLOUT.as_bytes());
    }

    /// What libzstd's own decoder (Codex's reader) accepts: frames back to
    /// back are one text, and a skippable frame is stepped over.
    #[test]
    fn frames_back_to_back_and_skippable_frames_read_as_libzstd_reads_them() {
        let mut bytes = ROLLOUT_ZST.to_vec();
        bytes.extend_from_slice(&0x184D_2A50u32.to_le_bytes());
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(b"pad");
        bytes.extend(compress(b"{\"more\":1}\n"));
        assert_eq!(decode(&bytes).expect("decode"), format!("{ROLLOUT}{{\"more\":1}}\n").into_bytes());

        assert!(
            decode(&bytes[..bytes.len() - 3]).is_err(),
            "a cut-off frame is an error, not a shorter text"
        );
        assert!(decode(b"{\"plain\":true}\n").is_err(), "and a plain file is not a frame");
    }

    #[test]
    fn the_two_forms_of_a_path_name_each_other() {
        let plain = Path::new("/s/rollout-1-a.jsonl");
        let compressed = Path::new("/s/rollout-1-a.jsonl.zst");
        assert!(is_compressed(compressed) && !is_compressed(plain));
        assert_eq!(compressed_path(plain), compressed);
        assert_eq!(compressed_path(compressed), compressed);
        assert_eq!(plain_path(compressed), plain);
        assert_eq!(plain_path(plain), plain);
        assert_eq!(other_form(plain).as_deref(), Some(compressed));
        assert_eq!(other_form(compressed).as_deref(), Some(plain));
        assert_eq!(other_form(Path::new("/s/notes.txt")), None, "only a transcript has another form");
    }

    /// Codex's rule: the plain file when it is there, whichever form the path
    /// names; the compressed one when it is the only one; nothing otherwise.
    #[tokio::test]
    async fn the_plain_form_is_preferred_and_the_compressed_one_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plain = dir.path().join("rollout-1-a.jsonl");
        let compressed = compressed_path(&plain);
        assert!(existing(&plain).await.is_none());

        std::fs::write(&compressed, ROLLOUT_ZST).expect("zst");
        let (found, metadata) = existing(&plain).await.expect("the compressed form");
        assert_eq!(found, compressed);
        assert_eq!(
            content_len(&found, &metadata).await,
            ROLLOUT.len() as u64,
            "the length of the text, from the header"
        );

        std::fs::write(&plain, ROLLOUT).expect("plain");
        assert_eq!(existing(&plain).await.map(|(path, _)| path), Some(plain.clone()));
        assert_eq!(
            existing(&compressed).await.map(|(path, _)| path),
            Some(plain.clone()),
            "whichever form is named"
        );
    }

    /// A rollout Codex compressed, or decompressed, after it was found is read
    /// in the form it has now.
    #[tokio::test]
    async fn a_transcript_that_changed_form_is_followed_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plain = dir.path().join("rollout-1-a.jsonl");
        let compressed = compressed_path(&plain);

        async fn text(path: &Path) -> io::Result<String> {
            let mut text = String::new();
            open_text(path).await?.read_to_string(&mut text).await?;
            Ok(text)
        }

        std::fs::write(&compressed, ROLLOUT_ZST).expect("zst");
        assert_eq!(text(&plain).await.expect("read"), ROLLOUT, "compressed since it was found");
        std::fs::remove_file(&compressed).expect("rm");
        std::fs::write(&plain, ROLLOUT).expect("plain");
        assert_eq!(text(&compressed).await.expect("read"), ROLLOUT, "decompressed since it was found");
        std::fs::remove_file(&plain).expect("rm");
        assert_eq!(text(&plain).await.expect_err("gone").kind(), io::ErrorKind::NotFound);
    }

    /// A tail is decoded once per file: the next ask it reaches back far
    /// enough for is answered from it, and one for more, or of a file that
    /// changed, is decoded anew.
    #[tokio::test]
    async fn a_tail_is_decoded_once_per_file() {
        async fn tail(path: &Path, max_bytes: u64) -> io::Result<(Vec<u8>, bool)> {
            match open(path).await? {
                Opened::Compressed(file) => file.tail(max_bytes).await,
                Opened::Plain(_) => panic!("a compressed transcript"),
            }
        }
        fn end(text: &str, len: usize) -> (Vec<u8>, bool) {
            (text.as_bytes()[text.len() - len..].to_vec(), true)
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout-1-a.jsonl.zst");
        let text: String = (0..5_000).map(|n| format!("{{\"n\":{n}}}\n")).collect();
        std::fs::write(&path, compress(text.as_bytes())).expect("zst");
        assert_eq!(tail(&path, 1_000).await.expect("tail"), end(&text, 1_000));

        // The same length and mtime, and nothing a decoder could read: what
        // is answered now was kept.
        let mtime = std::fs::metadata(&path).and_then(|metadata| metadata.modified()).expect("mtime");
        let len = std::fs::metadata(&path).expect("stat").len() as usize;
        std::fs::write(&path, vec![0u8; len]).expect("garbage");
        std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|file| file.set_modified(mtime))
            .expect("mtime");
        assert_eq!(tail(&path, 100).await.expect("kept"), end(&text, 100));
        assert!(tail(&path, 2_000).await.is_err(), "more than was kept is decoded again");

        let longer: String = (0..6_000).map(|n| format!("{{\"m\":{n}}}\n")).collect();
        std::fs::write(&path, compress(longer.as_bytes())).expect("zst");
        assert_eq!(tail(&path, 100).await.expect("tail"), end(&longer, 100), "a changed file is decoded anew");
        assert_eq!(tail(&path, u64::MAX).await.expect("tail"), (longer.clone().into_bytes(), false));
        assert_eq!(
            tail(&path, 2_000).await.expect("kept"),
            end(&longer, 2_000),
            "the whole text answers any ask"
        );
    }

    /// Longer than a chunk, a window and a block, so every hand-over and
    /// every frame boundary inside the decoder is crossed.
    #[tokio::test]
    async fn a_long_transcript_streams_line_for_line() {
        let lines: Vec<String> = (0..60_000).map(|n| format!(r#"{{"n":{n},"pad":"{}"}}"#, "x".repeat(n % 97))).collect();
        let text = lines.join("\n") + "\n";
        assert!(text.len() > 3 * 1024 * 1024);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout-1-a.jsonl.zst");
        std::fs::write(&path, compress(text.as_bytes())).expect("zst");

        let mut stream = open_text(&path).await.expect("open");
        let mut read = Vec::new();
        let mut line = String::new();
        while stream.read_line(&mut line).await.expect("read") > 0 {
            read.push(line.trim_end().to_string());
            line.clear();
        }
        assert_eq!(read, lines);
    }
}
