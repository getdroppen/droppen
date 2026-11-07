//! The server's end of a shim link.
//!
//! [`connect`] does the whole handshake — open the socket, say hello at a byte
//! offset, read the reply — before it returns, because everything the caller
//! must decide next depends on the reply. The corpse guards in particular:
//! `shell_pid` is how adoption tells a shim that is minding a live shell from
//! one that is minding a dead one.

use std::path::Path;
use std::time::Duration;

use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use super::proto::{self, Frame, Hello, Hello2};

/// How long a connect and its handshake may take.
///
/// A shim that is alive answers in microseconds; one that has wedged must not be
/// allowed to hold up a boot that has a whole instance directory to scan.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a link could not be established.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    /// The socket is not there, or refused the connection.
    #[error("cannot reach the pty shim: {0}")]
    Connect(std::io::Error),
    /// The socket accepted but the handshake did not complete.
    #[error("the pty shim did not complete its handshake")]
    Handshake,
}

/// A live link to one shim.
pub struct Link {
    /// The shell's pid, which is also its process group.
    pub shell_pid: u32,
    /// How much output the shell has produced in its whole life.
    pub total: u64,
    inbound: mpsc::UnboundedReceiver<Frame>,
    writer: OwnedWriteHalf,
    /// The task that owns the read half. Aborted with this struct, so dropping a
    /// link really does close the socket — a reader parked on a shim that has
    /// nothing to say would otherwise hold it open, and the shim would wait out
    /// its whole linger for a client that is already gone.
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link")
            .field("shell_pid", &self.shell_pid)
            .field("total", &self.total)
            .finish()
    }
}

/// The kernel's answer to "who is on the other end?" for a shim socket.
///
/// A unix socket has no access control of its own: the folder's mode keeps
/// other users out, and a folder can be squatted before this process looks at
/// it. The peer's uid is the one fact about the connection that whoever bound
/// the socket cannot arrange.
pub trait PeerUserExt {
    /// Whether the process at the other end runs as this user.
    fn is_same_user(&self) -> bool;
}

impl PeerUserExt for UnixStream {
    fn is_same_user(&self) -> bool {
        // SAFETY: `getuid` reads a process property and cannot fail.
        let me = unsafe { libc::getuid() };
        self.peer_cred().is_ok_and(|cred| cred.uid() == me)
    }
}

/// Connect to a shim and resume at `cursor`.
pub async fn connect(socket: &Path, cursor: u64) -> Result<Link, LinkError> {
    let stream = tokio::time::timeout(HANDSHAKE_TIMEOUT, UnixStream::connect(socket))
        .await
        .map_err(|_| LinkError::Connect(std::io::ErrorKind::TimedOut.into()))?
        .map_err(LinkError::Connect)?;
    // Before a byte — the hello included, which names the shell pid the caller
    // will guard on — is read from or written to whoever answered.
    if !stream.is_same_user() {
        return Err(LinkError::Connect(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("refusing the pty shim socket {}: it is served by another user", socket.display()),
        )));
    }
    let (mut reader, mut writer) = stream.into_split();

    let handshake = async {
        proto::write_frame(&mut writer, &Frame::json(proto::HELLO, &Hello { cursor }))
            .await
            .ok()?;
        // The shim answers `h` first, always: the gap frame and the backlog come
        // after it, so reading one frame here cannot swallow output.
        let frame = proto::read_frame(&mut reader).await.ok()??;
        (frame.kind == proto::HELLO).then(|| frame.parse::<Hello2>())?
    };
    let hello = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake)
        .await
        .map_err(|_| LinkError::Handshake)?
        .ok_or(LinkError::Handshake)?;

    // A dedicated reader from here on: a framed read is not cancellation-safe,
    // so it must never be one arm of the caller's `select!`.
    let (inbound_tx, inbound) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        loop {
            match proto::read_frame(&mut reader).await {
                Ok(Some(frame)) => {
                    if inbound_tx.send(frame).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    tracing::debug!(%error, "pty shim link ended");
                    break;
                }
            }
        }
    });

    Ok(Link {
        shell_pid: hello.shell_pid,
        total: hello.total,
        inbound,
        writer,
        reader: task,
    })
}

impl Link {
    /// The next frame the shim sent, or `None` once the link is gone.
    ///
    /// Cancellation-safe: the framing happens in the reader task, so what is
    /// polled here is a channel.
    pub async fn recv(&mut self) -> Option<Frame> {
        self.inbound.recv().await
    }

    /// Send one frame. An error means the link is gone and must be rebuilt.
    pub async fn send(&mut self, frame: &Frame) -> std::io::Result<()> {
        proto::write_frame(&mut self.writer, frame).await
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Link {
    /// A link whose inbound `frames` are all queued before anything reads them,
    /// for a test that needs a burst to be there at once. Writes go to a socket
    /// nobody reads. The sender comes back so the link does not end when the
    /// frames run out.
    pub fn preloaded(frames: Vec<Frame>) -> std::io::Result<(Link, mpsc::UnboundedSender<Frame>)> {
        let (ours, theirs) = UnixStream::pair()?;
        let (_, writer) = ours.into_split();
        let (inbound_tx, inbound) = mpsc::unbounded_channel();
        for frame in frames {
            let _ = inbound_tx.send(frame);
        }
        // Holds the far end open for as long as the link lives.
        let reader = tokio::spawn(async move {
            let _theirs = theirs;
            std::future::pending::<()>().await;
        });
        let link = Link {
            shell_pid: 0,
            total: 0,
            inbound,
            writer,
            reader,
        };
        Ok((link, inbound_tx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connecting_to_nothing_is_an_error_rather_than_a_hang() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error = connect(&dir.path().join("absent.sock"), 0).await.expect_err("no socket");
        assert!(matches!(error, LinkError::Connect(_)));
    }

    /// A socket this process made answers "same user". The other answer — a
    /// different uid on the other end — needs a second account to bind a
    /// socket, which an unprivileged test cannot conjure, so the refusal is
    /// documented rather than faked; what is tested here is that the check
    /// comes from the kernel's credential and not from a stub.
    #[tokio::test]
    async fn a_socket_this_process_made_belongs_to_this_user() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ours.sock");
        let listener = tokio::net::UnixListener::bind(&path).expect("bind");
        let client = UnixStream::connect(&path).await.expect("connect");
        assert!(client.is_same_user());
        let (accepted, _) = listener.accept().await.expect("accept");
        assert!(accepted.is_same_user());
    }

    /// A socket that accepts and then says nothing must not hold a boot up.
    #[tokio::test(start_paused = true)]
    async fn a_silent_shim_fails_the_handshake_instead_of_stalling_adoption() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("mute.sock");
        let listener = tokio::net::UnixListener::bind(&path).expect("bind");
        tokio::spawn(async move {
            let held = listener.accept().await;
            // Hold the connection open, saying nothing at all.
            std::future::pending::<()>().await;
            drop(held);
        });

        let error = connect(&path, 0).await.expect_err("a mute shim");
        assert!(matches!(error, LinkError::Handshake));
    }
}
