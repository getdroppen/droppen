//! The frame the server and a shim speak, and the sidecar shapes around it.
//!
//! One length-prefixed frame: `[1 byte type][u32 LE length][payload]`. Nothing
//! else — no version handshake, no capability negotiation, no compression. The
//! protocol is deliberately frozen: a shim outlives the server that spawned it,
//! so after an upgrade a *new* server talks to an *old* shim. Anything that can
//! be negotiated can be negotiated wrongly, and the failure mode is a terminal
//! nobody can reattach to.
//!
//! Frames the server sends:
//!
//! | type | payload                       | meaning                              |
//! |------|-------------------------------|--------------------------------------|
//! | `h`  | `{"cursor": u64}`             | attach, resuming at this byte offset |
//! | `i`  | raw bytes                     | keystrokes for the shell             |
//! | `r`  | `{"cols": u16, "rows": u16}`  | the window resized                   |
//! | `s`  | empty                         | send me a repaint of the screen      |
//! | `k`  | empty                         | terminate the shell                  |
//!
//! Frames the shim sends:
//!
//! | type | payload                          | meaning                          |
//! |------|----------------------------------|----------------------------------|
//! | `h`  | `{"shell_pid": u32, "total": u64}`| attached; here is what I am      |
//! | `o`  | raw bytes                        | output, starting at your cursor  |
//! | `s`  | raw bytes                        | a vt100 repaint of the screen    |
//! | `g`  | `{"from": u64, "to": u64}`       | that much output is gone for good|
//! | `x`  | `{"code": i32}`                  | the shell exited                 |
//!
//! Two of those may arrive in pieces, and both concatenate: `o` because a
//! backlog can be larger than any one frame, and `s` because a repaint of a
//! large enough window can be — the shim splits either at [`MAX_PAYLOAD`] rather
//! than sending a frame the receiver would refuse. Nothing marks the last piece,
//! and nothing needs to: both are opaque bytes on their way to a terminal, so a
//! receiver that forwards each payload as it comes has already reassembled them.
//! The pieces are cut on character boundaries, for the receivers that decode
//! each payload on its own before forwarding it.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Attach (server) / attached (shim).
pub const HELLO: u8 = b'h';
/// Keystrokes for the shell.
pub const INPUT: u8 = b'i';
/// The window resized.
pub const RESIZE: u8 = b'r';
/// Ask for (server) / carry (shim) a screen repaint.
pub const SNAPSHOT: u8 = b's';
/// Terminate the shell.
pub const KILL: u8 = b'k';
/// Output bytes.
pub const OUTPUT: u8 = b'o';
/// Output that fell out of the ring before anyone read it.
pub const GAP: u8 = b'g';
/// The shell exited.
pub const EXIT: u8 = b'x';

/// The largest payload either side will accept.
///
/// Sized off the ring plus room for a repaint of a very large window, so a
/// backlog can always be delivered in one frame if the sender wants to. Its real
/// job is to stop a corrupt length prefix from turning into a multi-gigabyte
/// allocation.
pub const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

/// The fixed part of every frame: type byte plus length.
pub const HEADER_LEN: usize = 5;

/// One protocol frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// One of the type constants above.
    pub kind: u8,
    /// The bytes after the header; JSON for the control frames, raw otherwise.
    pub payload: Vec<u8>,
}

impl Frame {
    /// A frame carrying raw bytes.
    pub fn raw(kind: u8, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            kind,
            payload: payload.into(),
        }
    }

    /// A frame carrying a JSON control message.
    pub fn json<T: Serialize>(kind: u8, value: &T) -> Self {
        Self {
            kind,
            // Every control shape in this module is a struct of scalars, so
            // serialization cannot fail; an empty payload is still parseable as
            // "malformed" by the peer rather than a panic here.
            payload: serde_json::to_vec(value).unwrap_or_default(),
        }
    }

    /// A frame with no payload.
    pub fn empty(kind: u8) -> Self {
        Self { kind, payload: Vec::new() }
    }

    /// Parse the payload as one of the control shapes, or `None` if it is not.
    pub fn parse<T: for<'de> Deserialize<'de>>(&self) -> Option<T> {
        serde_json::from_slice(&self.payload).ok()
    }

    /// The frame as it goes on the wire.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len());
        out.push(self.kind);
        out.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.payload);
        out
    }
}

/// `h`, server to shim.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Hello {
    /// The absolute output offset the server has already consumed.
    pub cursor: u64,
}

/// `h`, shim to server.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Hello2 {
    /// The shell's pid, which is also its process group id.
    pub shell_pid: u32,
    /// How many bytes the shell has produced since it started.
    pub total: u64,
}

/// `r`, server to shim.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Resize {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
}

/// `g`, shim to server.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Gap {
    /// First offset that is gone.
    pub from: u64,
    /// First offset that is still held.
    pub to: u64,
}

impl Gap {
    /// How many bytes were lost.
    pub fn lost(&self) -> u64 {
        self.to.saturating_sub(self.from)
    }
}

/// `x`, shim to server.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Exit {
    /// The shell's exit status, or `-1` when there was none to read.
    pub code: i32,
}

/// `<id>.exit`: the shim's tombstone, written before the `x` frame goes out.
///
/// The file is the authority, not the frame: a server that was not attached when
/// the shell died learns about it here, at the next boot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tombstone {
    /// The shell's exit status.
    pub exit_code: i32,
    /// When it exited, ISO-8601.
    pub at: String,
}

/// `<id>.meta.json`: the server's own record of a terminal.
///
/// Server-owned, written before the shim is spawned. This — not the socket — is
/// what `GET /api/terminals` is rebuilt from after a restart, which is why
/// adoption reads it first and treats a shim without one as an orphan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    /// The terminal id; also the sidecar filename stem.
    pub id: String,
    /// Display name.
    pub title: String,
    /// Working directory the shell started in.
    pub cwd: String,
    /// Creation order, so adopted tabs come back where the user left them.
    pub created_seq: u64,
    /// The program the shim was told to run.
    pub shell: String,
    /// When it was created, ISO-8601.
    ///
    /// `TerminalInfo` carries it, so without it every adopted terminal would
    /// claim to have been created at the moment the server restarted.
    pub created_at: String,
}

/// Why a frame could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    /// The transport failed.
    #[error("shim link: {0}")]
    Io(#[from] std::io::Error),
    /// The peer announced a payload larger than [`MAX_PAYLOAD`].
    #[error("shim link: frame of {0} bytes is too large")]
    TooLarge(usize),
}

/// Read one frame, or `None` at a clean end of stream.
///
/// Not cancellation-safe — a frame half-read is a frame lost — which is why the
/// server runs this in a task of its own and hands frames on over a channel
/// rather than polling it inside a `select!`.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Option<Frame>, ProtoError> {
    let mut header = [0u8; HEADER_LEN];
    let mut filled = 0;
    while filled < HEADER_LEN {
        match reader.read(&mut header[filled..]).await? {
            // A stream that ends between frames is a peer that went away
            // cleanly; one that ends mid-header is a truncated frame.
            0 if filled == 0 => return Ok(None),
            0 => return Err(ProtoError::Io(std::io::ErrorKind::UnexpectedEof.into())),
            read => filled += read,
        }
    }
    let kind = header[0];
    let len = u32::from_le_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if len > MAX_PAYLOAD {
        return Err(ProtoError::TooLarge(len));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    Ok(Some(Frame { kind, payload }))
}

/// Write one frame and flush it.
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &Frame) -> std::io::Result<()> {
    writer.write_all(&frame.encode()).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire is a byte stream, so a frame may arrive in any number of pieces
    /// — and two frames may arrive in one. Both have to decode identically.
    #[tokio::test]
    async fn frames_round_trip_however_the_bytes_are_split() {
        let frames = vec![
            Frame::json(HELLO, &Hello { cursor: 42 }),
            Frame::raw(OUTPUT, b"hello \xf0\x9f\x91\x8b".to_vec()),
            Frame::empty(SNAPSHOT),
            Frame::json(GAP, &Gap { from: 7, to: 99 }),
            Frame::raw(OUTPUT, vec![0u8; 300]),
            Frame::json(EXIT, &Exit { code: 7 }),
        ];
        let mut wire = Vec::new();
        for frame in &frames {
            wire.extend_from_slice(&frame.encode());
        }

        for chunk in [1usize, 2, 5, 7, 64, 4096] {
            let (mut client, mut server) = tokio::io::duplex(8);
            let wire = wire.clone();
            tokio::spawn(async move {
                for piece in wire.chunks(chunk) {
                    server.write_all(piece).await.expect("write piece");
                }
                server.shutdown().await.expect("shutdown");
            });

            let mut decoded = Vec::new();
            while let Some(frame) = read_frame(&mut client).await.expect("decode") {
                decoded.push(frame);
            }
            assert_eq!(decoded, frames, "split into {chunk}-byte pieces");
        }
    }

    #[tokio::test]
    async fn a_stream_that_ends_between_frames_is_clean_and_one_that_ends_inside_is_not() {
        let (mut client, server) = tokio::io::duplex(64);
        drop(server);
        assert!(read_frame(&mut client).await.expect("clean eof").is_none());

        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_all(&[OUTPUT, 9]).await.expect("half a header");
        drop(server);
        assert!(read_frame(&mut client).await.is_err(), "a truncated header is an error");
    }

    #[tokio::test]
    async fn an_absurd_length_prefix_is_refused_rather_than_allocated() {
        let (mut client, mut server) = tokio::io::duplex(64);
        let mut header = vec![OUTPUT];
        header.extend_from_slice(&u32::MAX.to_le_bytes());
        server.write_all(&header).await.expect("write header");
        assert!(matches!(read_frame(&mut client).await, Err(ProtoError::TooLarge(_))));
    }

    #[test]
    fn a_control_payload_that_is_not_the_shape_expected_is_none_rather_than_a_panic() {
        assert!(Frame::raw(HELLO, b"not json".to_vec()).parse::<Hello>().is_none());
        assert_eq!(Frame::json(HELLO, &Hello { cursor: 5 }).parse::<Hello>().expect("hello").cursor, 5);
        assert_eq!(Gap { from: 10, to: 25 }.lost(), 15);
        assert_eq!(Gap { from: 25, to: 10 }.lost(), 0, "a nonsensical gap is zero, never a huge number");
    }
}
