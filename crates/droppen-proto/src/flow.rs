//! Hub-level flow control on a channel: the `CREDIT` frames of [`crate::frame`].
//!
//! Every channel (a hub `ch` other than 0) has a window in each direction. A
//! sender may have at most [`INITIAL_CREDIT`] bytes of `DATA` payload that the
//! receiver has not yet handed back with a `CREDIT` frame; the receiver hands
//! credit back as soon as it has *read* the data (decrypted and demultiplexed
//! it), in batches of half a window so `CREDIT`s stay rare. Application-level
//! slowness is the sub-stream windows' business ([`crate::mux`]), not this one's:
//! this window bounds only what sits in the hub and on the wire.
//!
//! The hub enforces it rather than trusting either end: it tracks the same
//! [`SendWindow`] for both directions of every channel it relays, closes a
//! channel whose sender overruns it, and refuses a grant that would take a
//! window above [`INITIAL_CREDIT`] (a receiver can only ever hand back what it
//! was sent). So what one channel can make the hub hold is bounded by the
//! window, whatever the two ends do.

use thiserror::Error;

/// What a sender may have outstanding on one channel, in one direction.
pub const INITIAL_CREDIT: u32 = 512 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FlowError {
    #[error("{len} bytes sent with only {available} bytes of credit")]
    Overrun { len: usize, available: u64 },
    #[error("a grant of {granted} bytes would take the window past {INITIAL_CREDIT}")]
    OverGranted { granted: u32 },
}

/// The sender's side of a window (and the hub's copy of it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendWindow {
    available: u64,
}

impl Default for SendWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl SendWindow {
    pub fn new() -> Self {
        Self {
            available: u64::from(INITIAL_CREDIT),
        }
    }

    pub fn available(&self) -> u64 {
        self.available
    }

    pub fn can_send(&self, len: usize) -> bool {
        len as u64 <= self.available
    }

    /// Spend `len` bytes of credit, or refuse without spending any.
    pub fn consume(&mut self, len: usize) -> Result<(), FlowError> {
        if !self.can_send(len) {
            return Err(FlowError::Overrun {
                len,
                available: self.available,
            });
        }
        self.available -= len as u64;
        Ok(())
    }

    /// The receiver handed `bytes` back.
    pub fn grant(&mut self, bytes: u32) -> Result<(), FlowError> {
        let next = self.available + u64::from(bytes);
        if next > u64::from(INITIAL_CREDIT) {
            return Err(FlowError::OverGranted { granted: bytes });
        }
        self.available = next;
        Ok(())
    }
}

/// The receiver's side: how much has been read and not yet handed back.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RecvWindow {
    unacked: u64,
}

impl RecvWindow {
    pub fn new() -> Self {
        Self::default()
    }

    /// `len` bytes were read; the credit to send back now, if any.
    pub fn received(&mut self, len: usize) -> Option<u32> {
        self.unacked += len as u64;
        if self.unacked >= u64::from(INITIAL_CREDIT / 2) {
            let grant = self.unacked.min(u64::from(INITIAL_CREDIT)) as u32;
            self.unacked -= u64::from(grant);
            Some(grant)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sender_stops_at_the_window_and_resumes_on_credit() {
        let mut send = SendWindow::new();
        let mut recv = RecvWindow::new();
        let chunk = 64 * 1024 - 7;
        let mut sent = 0usize;
        let mut grants = Vec::new();
        while send.can_send(chunk) {
            send.consume(chunk).unwrap();
            sent += chunk;
            if let Some(g) = recv.received(chunk) {
                grants.push(g);
            }
        }
        assert!(sent <= INITIAL_CREDIT as usize);
        assert!(matches!(send.consume(chunk), Err(FlowError::Overrun { .. })));
        for g in grants {
            send.grant(g).unwrap();
        }
        assert!(send.can_send(chunk));
    }

    #[test]
    fn a_grant_cannot_inflate_the_window() {
        let mut send = SendWindow::new();
        assert_eq!(send.grant(1), Err(FlowError::OverGranted { granted: 1 }));
        send.consume(100).unwrap();
        send.grant(100).unwrap();
        assert_eq!(send.available(), u64::from(INITIAL_CREDIT));
    }
}
