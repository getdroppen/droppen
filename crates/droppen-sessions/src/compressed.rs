//! Transcripts a CLI has compressed where they lie.
//!
//! Codex compresses a rollout nobody has written to for a week: the file
//! becomes `<name>.jsonl.zst` beside it, one zstd frame, and the plain
//! `<name>.jsonl` is deleted. Its state database goes on naming the plain
//! file, and Codex decompresses the rollout back to that name before it next
//! appends (`codex-rs/rollout/src/compression.rs`). Either form can be the one
//! on disk, and either can replace the other at any moment.
//!
//! So a transcript is named by its plain path whichever form is on disk: its
//! row, its cache entries, its fingerprint chain and its cloud copy stay the
//! same session through a change of form. Only what stats or opens the file
//! finds the form that is there ([`locate`], by Codex's own rule). Its size is
//! the length of its text ([`content_len`]), which is the plain file's, and
//! Codex keeps the mtime both ways, so its [`crate::file_signature`] does not
//! move either: a change of form is invisible above the readers.
//!
//! A compressed one is read through a pure-Rust zstd decoder ([`ruzstd`]):
//! nothing in the server links a C library. It is cold, and a zstd frame can
//! only be read from its start, so it is read whole every time it is read,
//! streamed from a blocking thread a chunk at a time. What is asked of it on
//! every poll, its tail, is kept ([`tail`]).

use std::collections::VecDeque;
use std::io::{self, BufRead, Read};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use ruzstd::decoding::StreamingDecoder;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};
use tokio::sync::mpsc;
use tokio_util::io::StreamReader;

use crate::util::FileCache;

/// What a compressed transcript's name ends in.
const COMPRESSED_SUFFIX: &str = ".jsonl.zst";

/// How much decoded text one hand-over from the decoding thread carries.
const CHUNK: usize = 64 * 1024;

/// How many decoded chunks may wait for a slow reader before the decoder
/// waits for it: what bounds a streamed transcript's memory.
const CHUNKS_IN_FLIGHT: usize = 4;

/// The longest a zstd frame header is: the magic number and at most 14 bytes.
const FRAME_HEADER_MAX: u64 = 18;

/// Whether `path` names a compressed transcript (`*.jsonl.zst`).
pub fn is_compressed(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(COMPRESSED_SUFFIX))
}

/// The plain form of a transcript path: `path` without its `.zst`.
pub fn plain_path(path: &Path) -> PathBuf {
    match path.file_name().and_then(|name| name.to_str()).filter(|_| is_compressed(path)) {
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

/// The file the transcript `path` names, in the form it is on disk, and its
/// metadata.
///
/// Codex's own rule (`existing_rollout_with_metadata_sync`), whichever form
/// `path` names: the plain file when it is there, otherwise its compressed
/// form. Both are on disk only while one is replacing the other, and Codex
/// reads the plain one. Only a `.jsonl` has a compressed form. When neither is
/// there, the error says why: a form that could not be looked at, before one
/// that is not there.
pub async fn locate(path: &Path) -> io::Result<(PathBuf, std::fs::Metadata)> {
    let plain = plain_path(path);
    let compressed = compressed_path(&plain);
    let candidates = [Some(plain), is_compressed(&compressed).then_some(compressed)];
    let mut refused = None;
    for candidate in candidates.into_iter().flatten() {
        match tokio::fs::metadata(&candidate).await {
            Ok(metadata) if metadata.is_file() => return Ok((candidate, metadata)),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                refused.get_or_insert(error);
            }
        }
    }
    Err(refused.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("no transcript at {}, plain or compressed", path.display()),
        )
    }))
}

/// The content lengths read off compressed transcripts' frame headers, by the
/// mtime and size of the file they were read from.
static CONTENT_LENS: LazyLock<FileCache<u64>> = LazyLock::new(|| FileCache::new(4096));

/// How long the transcript [`locate`] found at `path` is, as text: its length
/// on disk, or for a compressed one the length its frame header says it
/// decompresses to — the plain file's length, so a row's size and a file's
/// signature are the same in either form.
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

/// The decompressed length the frame header of `path` states, if it does.
async fn frame_content_len(path: &Path) -> Option<u64> {
    let mut header = Vec::new();
    tokio::fs::File::open(path)
        .await
        .ok()?
        .take(FRAME_HEADER_MAX)
        .read_to_end(&mut header)
        .await
        .ok()?;
    let mut frame = ruzstd::decoding::FrameDecoder::new();
    frame.init(header.as_slice()).ok()?;
    // Zero is how the decoder says the header named no length.
    Some(frame.content_size()).filter(|len| *len > 0)
}

/// Open the transcript `path` names, in the form it is on disk ([`locate`]),
/// and say which file that is.
///
/// One compressed or decompressed between being found and being opened is
/// found again, once.
pub(crate) async fn open(path: &Path) -> io::Result<(PathBuf, tokio::fs::File)> {
    let mut again = false;
    loop {
        let (found, _) = locate(path).await?;
        match tokio::fs::File::open(&found).await {
            Err(error) if error.kind() == io::ErrorKind::NotFound && !again => again = true,
            opened => return opened.map(|file| (found, file)),
        }
    }
}

/// How much of a plain transcript one read takes from its file, for a reader
/// that reads the text to its end ([`crate::util::LossyLines`]).
///
/// Each read of a [`tokio::fs::File`] is a trip to a blocking thread and back,
/// and a [`tokio::io::BufReader`] reads 8 KiB at a time unless told otherwise.
/// A 480 MiB transcript was then 60,000 trips: most of what a whole parse of
/// it cost (0.7 s of 0.9 s on an M4), and readers reading at once slowed each
/// other down badly (six took 2.4 s, and 12–15 s of CPU). A MiB at a time is
/// 480 trips: the same read took 0.2 s, and six at once under half a second.
/// The buffer is held only while the file is read.
pub(crate) const WHOLE_READ: usize = 1024 * 1024;

/// How much of a plain transcript one read takes for a reader that stops
/// early ([`crate::util::read_head_lines`]): tokio's own default, so a head of
/// a few KiB is not read a MiB at a time.
pub(crate) const PART_READ: usize = 8 * 1024;

/// The text of the transcript `path` names, as a stream: a plain file as it
/// is, a compressed one decoded on a blocking thread a chunk at a time, so a
/// long rollout is never held whole. Opened as [`open`] opens it.
///
/// `buffer` is how much of a plain file one read takes: [`WHOLE_READ`] for a
/// reader that goes on to the end, [`PART_READ`] for one that stops early. A
/// compressed one is handed over a [`CHUNK`] at a time either way.
pub(crate) async fn open_text(path: &Path, buffer: usize) -> io::Result<Box<dyn AsyncBufRead + Send + Unpin>> {
    let (found, file) = open(path).await?;
    Ok(if is_compressed(&found) {
        Box::new(decoded(file.into_std().await))
    } else {
        Box::new(tokio::io::BufReader::with_capacity(buffer, file))
    })
}

/// What the decoding thread hands over.
enum Decoded {
    /// The next stretch of the text.
    Text(io::Cursor<Vec<u8>>),
    /// The text ended here, and the file with it.
    End,
    /// The file could not be read or decoded past here.
    Failed(io::Error),
}

/// `file`'s text, decoded on a blocking thread.
fn decoded(file: std::fs::File) -> impl AsyncBufRead + Send + Unpin {
    let (tx, rx) = mpsc::channel(CHUNKS_IN_FLIGHT);
    tokio::task::spawn_blocking(move || decode(file, &tx));
    text_of(rx)
}

/// The text a decoding thread hands over on `rx`, as a reader.
///
/// It ends where the thread says the text ends, and only there: a thread gone
/// without saying so (it panicked) is an error, never a shorter text — a
/// shorter text would be parsed, counted and remembered as the whole of it.
fn text_of(rx: mpsc::Receiver<Decoded>) -> impl AsyncBufRead + Send + Unpin {
    let chunks = futures_util::stream::unfold(Some(rx), |rx| async move {
        let mut rx = rx?;
        let error = match rx.recv().await {
            Some(Decoded::Text(text)) => return Some((Ok(text), Some(rx))),
            Some(Decoded::End) => return None,
            Some(Decoded::Failed(error)) => error,
            None => io::Error::new(io::ErrorKind::UnexpectedEof, "the decoder stopped before the end of the text"),
        };
        Some((Err(error), None))
    });
    StreamReader::new(Box::pin(chunks))
}

/// Decode `file` onto `tx` a chunk at a time, then say how it ended. Blocking.
///
/// Codex writes one frame and nothing after it (`encode_zstd_to_writer`), so
/// the text is that frame's, and bytes after it are an error: not a rollout
/// Codex wrote. The window is capped where libzstd's decoder caps it by
/// default, 128 MiB ([`ruzstd::decoding::DEFAULT_MAX_WINDOW_SIZE`]); Codex
/// compresses at level 3, whose window is at most 2 MiB.
fn decode(file: std::fs::File, tx: &mpsc::Sender<Decoded>) {
    let mut source = io::BufReader::new(file);
    let ended = (|| -> io::Result<()> {
        {
            let mut frame = StreamingDecoder::new(&mut source).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            loop {
                let mut chunk = vec![0u8; CHUNK];
                let read = match frame.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                };
                chunk.truncate(read);
                if tx.blocking_send(Decoded::Text(io::Cursor::new(chunk))).is_err() {
                    // Nobody is reading any more.
                    return Ok(());
                }
            }
        }
        if !source.fill_buf()?.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "more after the zstd frame"));
        }
        Ok(())
    })();
    let _ = tx.blocking_send(match ended {
        Ok(()) => Decoded::End,
        Err(error) => Decoded::Failed(error),
    });
}

/// How much of a compressed transcript's end is decoded at the least: the most
/// any reader asks of one (Codex's model reading, 1 MiB), so every ask a
/// session view makes is answered from one decode.
const TAIL_MIN: u64 = 1024 * 1024;

/// How many compressed transcripts' tails are kept: a few session views' worth.
/// The list asks no tail of a compressed transcript on every pass; what it
/// asks of a subagent's is kept by its reader, against the file.
const TAILS_HELD: usize = 4;

/// The end of a compressed transcript's text, decoded and kept.
struct Tail {
    path: PathBuf,
    /// The file's mtime and length when it was decoded.
    mark: (f64, u64),
    /// What the decode gave: the last `asked` bytes and whether text before
    /// them was left out, or why there was no text — kept too, so a damaged
    /// file is not decoded again on every poll.
    read: Result<(u64, Vec<u8>, bool), (io::ErrorKind, String)>,
}

/// The tails decoded last, least recently used first.
static TAILS: LazyLock<Mutex<VecDeque<Tail>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));

/// The last `max_bytes` of the text of the compressed transcript `found`,
/// opened as `file`, and whether anything before them was left out.
///
/// The whole text is decoded to reach its end, so the end is kept: a session
/// view asks its transcript's tail for the model and the context every two
/// seconds, of a file that does not change.
pub(crate) async fn tail(found: &Path, file: tokio::fs::File, max_bytes: u64) -> io::Result<(Vec<u8>, bool)> {
    let metadata = file.metadata().await?;
    let mark = (droppen_base::time::mtime_ms(&metadata), metadata.len());
    if let Some(kept) = kept_tail(found, mark, max_bytes) {
        return kept;
    }
    let asked = max_bytes.max(TAIL_MIN);
    let read = last_bytes(decoded(file.into_std().await), asked)
        .await
        .map(|(bytes, cut)| (asked, bytes, cut))
        .map_err(|error| (error.kind(), error.to_string()));
    let answer = answer(&read, max_bytes);
    let mut tails = TAILS.lock().expect("tail cache poisoned");
    tails.retain(|tail| tail.path != found);
    if tails.len() == TAILS_HELD {
        tails.pop_front();
    }
    tails.push_back(Tail {
        path: found.to_path_buf(),
        mark,
        read,
    });
    answer
}

/// What a kept tail of `path` as it was at `mark` says about its last
/// `max_bytes`, when it reaches back that far.
fn kept_tail(path: &Path, mark: (f64, u64), max_bytes: u64) -> Option<io::Result<(Vec<u8>, bool)>> {
    let mut tails = TAILS.lock().expect("tail cache poisoned");
    let at = tails.iter().position(|tail| {
        tail.path == path
            && tail.mark == mark
            && match &tail.read {
                Ok((asked, _, cut)) => *asked >= max_bytes || !cut,
                Err(_) => true,
            }
    })?;
    let tail = tails.remove(at)?;
    let kept = answer(&tail.read, max_bytes);
    tails.push_back(tail);
    Some(kept)
}

/// The last `max_bytes` of what a decode gave.
fn answer(read: &Result<(u64, Vec<u8>, bool), (io::ErrorKind, String)>, max_bytes: u64) -> io::Result<(Vec<u8>, bool)> {
    match read {
        Ok((_, bytes, cut)) => {
            let skip = bytes.len().saturating_sub(usize::try_from(max_bytes).unwrap_or(usize::MAX));
            Ok((bytes[skip..].to_vec(), *cut || skip > 0))
        }
        Err((kind, message)) => Err(io::Error::new(*kind, message.clone())),
    }
}

/// The last `keep` bytes of `text`, and whether anything before them was left
/// out.
async fn last_bytes(mut text: impl AsyncBufRead + Unpin, keep: u64) -> io::Result<(Vec<u8>, bool)> {
    let keep = usize::try_from(keep).unwrap_or(usize::MAX);
    let mut tail = Vec::new();
    let mut cut = false;
    loop {
        let chunk = text.fill_buf().await?;
        if chunk.is_empty() {
            break;
        }
        let read = chunk.len();
        tail.extend_from_slice(chunk);
        text.consume(read);
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

    /// The text of `bytes`, written as a compressed rollout in `dir`.
    async fn text_of_file(dir: &Path, bytes: &[u8]) -> io::Result<String> {
        let path = dir.join("rollout-1-a.jsonl.zst");
        std::fs::write(&path, bytes)?;
        let mut text = String::new();
        open_text(&path, WHOLE_READ).await?.read_to_string(&mut text).await?;
        Ok(text)
    }

    #[tokio::test]
    async fn a_frame_libzstd_wrote_decodes_to_the_rollout() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(text_of_file(dir.path(), &ROLLOUT_ZST).await.expect("decode"), ROLLOUT);
    }

    /// Codex writes one whole frame and nothing else: a cut-off frame, bytes
    /// after it, and a file that is no frame at all are errors, never a text
    /// that is merely shorter.
    #[tokio::test]
    async fn anything_but_one_whole_frame_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut trailing = ROLLOUT_ZST.to_vec();
        trailing.extend(compress(b"{\"more\":1}\n"));
        for (bytes, what) in [
            (&ROLLOUT_ZST[..ROLLOUT_ZST.len() - 3], "a cut-off frame"),
            (trailing.as_slice(), "a second frame"),
            (b"{\"plain\":true}\n".as_slice(), "a plain file"),
        ] {
            let error = text_of_file(dir.path(), bytes).await.expect_err(what);
            assert_ne!(error.kind(), io::ErrorKind::NotFound, "{what}: {error}");
        }
    }

    /// Read a line at a time, as the transcript parser reads it, a cut-off
    /// frame still ends in an error after the lines it did hold.
    #[tokio::test]
    async fn a_cut_off_frame_read_line_by_line_ends_in_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let text: String = (0..20_000).map(|n| format!("{{\"n\":{n}}}\n")).collect();
        let frame = compress(text.as_bytes());
        let path = dir.path().join("rollout-1-a.jsonl.zst");
        std::fs::write(&path, &frame[..frame.len() / 2]).expect("zst");
        let mut lines = crate::util::LossyLines::open(&path).await.expect("open");
        let mut read = 0usize;
        let error = loop {
            match lines.next_line().await {
                Ok(Some(_)) => read += 1,
                Ok(None) => panic!("{read} lines and a clean end, from half a frame"),
                Err(error) => break error,
            }
        };
        assert!(read < 20_000, "{read} lines before the error");
        assert_ne!(error.kind(), io::ErrorKind::NotFound);
    }

    /// A decoding thread that stops without saying the text ended (it
    /// panicked) is an error to the reader, not the end of the text.
    #[tokio::test]
    async fn a_decoder_gone_without_an_end_is_an_error() {
        let (tx, rx) = mpsc::channel(CHUNKS_IN_FLIGHT);
        tx.send(Decoded::Text(io::Cursor::new(b"{\"n\":1}\n".to_vec()))).await.expect("send");
        drop(tx);
        let mut text = String::new();
        let error = text_of(rx).read_to_string(&mut text).await.expect_err("no end");
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);

        let (tx, rx) = mpsc::channel(CHUNKS_IN_FLIGHT);
        tx.send(Decoded::Text(io::Cursor::new(b"{\"n\":1}\n".to_vec()))).await.expect("send");
        tx.send(Decoded::End).await.expect("send");
        let mut text = String::new();
        text_of(rx).read_to_string(&mut text).await.expect("an end");
        assert_eq!(text, "{\"n\":1}\n");
    }

    /// The test-support encoder states the length as libzstd does, so
    /// [`content_len`] reads the same off either.
    #[tokio::test]
    async fn the_test_encoder_writes_frames_as_codex_does() {
        let dir = tempfile::tempdir().expect("tempdir");
        let text = ROLLOUT.repeat(1_000);
        assert_eq!(text_of_file(dir.path(), &compress(text.as_bytes())).await.expect("decode"), text);
        let path = dir.path().join("rollout-1-a.jsonl.zst");
        let metadata = std::fs::metadata(&path).expect("stat");
        assert_eq!(content_len(&path, &metadata).await, text.len() as u64);
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
        assert_eq!(plain_path(Path::new("/s/notes.zst")), Path::new("/s/notes.zst"));
    }

    /// Codex's rule: the plain file when it is there, whichever form the path
    /// names; the compressed one when it is the only one; and when neither
    /// is, why.
    #[tokio::test]
    async fn the_plain_form_is_preferred_and_the_compressed_one_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plain = dir.path().join("rollout-1-a.jsonl");
        let compressed = compressed_path(&plain);
        assert_eq!(locate(&plain).await.expect_err("neither").kind(), io::ErrorKind::NotFound);

        std::fs::write(&compressed, ROLLOUT_ZST).expect("zst");
        let (found, metadata) = locate(&plain).await.expect("the compressed form");
        assert_eq!(found, compressed);
        assert_eq!(content_len(&found, &metadata).await, ROLLOUT.len() as u64, "the length of the text");

        std::fs::write(&plain, ROLLOUT).expect("plain");
        assert_eq!(locate(&plain).await.expect("plain").0, plain);
        assert_eq!(locate(&compressed).await.expect("plain").0, plain, "whichever form is named");
        let other = dir.path().join("notes.txt");
        std::fs::write(compressed_path(&other), ROLLOUT_ZST).expect("zst");
        assert!(locate(&other).await.is_err(), "only a transcript has a compressed form");
    }

    /// A form that cannot be looked at says so, rather than reading as absent.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_form_that_cannot_be_looked_at_says_why() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).expect("dir");
        let plain = locked.join("rollout-1-a.jsonl");
        std::fs::write(&plain, ROLLOUT).expect("plain");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("lock");
        let error = locate(&plain).await.expect_err("locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).expect("unlock");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
    }

    /// A rollout Codex compressed, or decompressed, after it was named is read
    /// in the form it has now.
    #[tokio::test]
    async fn a_transcript_is_read_in_the_form_it_has_now() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plain = dir.path().join("rollout-1-a.jsonl");
        let compressed = compressed_path(&plain);

        async fn text(path: &Path) -> io::Result<String> {
            let mut text = String::new();
            open_text(path, WHOLE_READ).await?.read_to_string(&mut text).await?;
            Ok(text)
        }

        std::fs::write(&compressed, ROLLOUT_ZST).expect("zst");
        assert_eq!(text(&plain).await.expect("read"), ROLLOUT, "compressed");
        std::fs::remove_file(&compressed).expect("rm");
        std::fs::write(&plain, ROLLOUT).expect("plain");
        assert_eq!(text(&compressed).await.expect("read"), ROLLOUT, "decompressed");
        std::fs::remove_file(&plain).expect("rm");
        assert_eq!(text(&plain).await.expect_err("gone").kind(), io::ErrorKind::NotFound);
    }

    /// A tail is decoded once per file, at least [`TAIL_MIN`] of it, so the
    /// asks a session view makes are answered from it; one for more, or of a
    /// file that changed, is decoded anew; and a file that will not decode is
    /// not decoded again until it changes.
    #[tokio::test]
    async fn a_tail_is_decoded_once_per_file() {
        async fn tail_of(path: &Path, max_bytes: u64) -> io::Result<(Vec<u8>, bool)> {
            let (found, file) = open(path).await?;
            tail(&found, file, max_bytes).await
        }
        fn end(text: &str, len: usize) -> (Vec<u8>, bool) {
            (text.as_bytes()[text.len() - len..].to_vec(), true)
        }
        fn same_length_and_mtime(path: &Path, bytes: &[u8]) {
            let metadata = std::fs::metadata(path).expect("stat");
            let mut replaced = bytes.to_vec();
            replaced.resize(metadata.len() as usize, 0);
            std::fs::write(path, replaced).expect("replace");
            std::fs::File::options()
                .write(true)
                .open(path)
                .and_then(|file| file.set_modified(metadata.modified()?))
                .expect("mtime");
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout-1-a.jsonl.zst");
        let text: String = (0..200_000).map(|n| format!("{{\"n\":{n}}}\n")).collect();
        assert!(text.len() as u64 > TAIL_MIN);
        std::fs::write(&path, compress(text.as_bytes())).expect("zst");
        assert_eq!(tail_of(&path, 1_000).await.expect("tail"), end(&text, 1_000));

        // Nothing a decoder could read, at the same length and mtime: what is
        // answered now was kept.
        same_length_and_mtime(&path, b"");
        assert_eq!(tail_of(&path, 64 * 1024).await.expect("kept"), end(&text, 64 * 1024));
        assert!(tail_of(&path, 2 * TAIL_MIN).await.is_err(), "more than was kept is decoded again");
        // The failure is kept as well; a file that changed is decoded anew.
        same_length_and_mtime(&path, &compress(text.as_bytes()));
        assert!(tail_of(&path, 2 * TAIL_MIN).await.is_err(), "not decoded again");
        let longer: String = (0..6_000).map(|n| format!("{{\"m\":{n}}}\n")).collect();
        std::fs::write(&path, compress(longer.as_bytes())).expect("zst");
        assert_eq!(
            tail_of(&path, 100).await.expect("tail"),
            end(&longer, 100),
            "a changed file is decoded anew"
        );
        assert_eq!(tail_of(&path, u64::MAX).await.expect("whole"), (longer.into_bytes(), false));
    }

    /// Longer than a chunk, a window and a block, so every hand-over and
    /// every block boundary inside the decoder is crossed.
    #[tokio::test]
    async fn a_long_transcript_streams_line_for_line() {
        let lines: Vec<String> = (0..60_000).map(|n| format!(r#"{{"n":{n},"pad":"{}"}}"#, "x".repeat(n % 97))).collect();
        let text = lines.join("\n") + "\n";
        assert!(text.len() > 3 * 1024 * 1024);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout-1-a.jsonl.zst");
        std::fs::write(&path, compress(text.as_bytes())).expect("zst");

        let mut stream = open_text(&path, WHOLE_READ).await.expect("open");
        let mut read = Vec::new();
        let mut line = String::new();
        while stream.read_line(&mut line).await.expect("read") > 0 {
            read.push(line.trim_end().to_string());
            line.clear();
        }
        assert_eq!(read, lines);
    }
}
