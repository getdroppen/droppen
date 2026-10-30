//! The global feed's two inputs, for the subsystems that sit below it.
//!
//! A route that changes what the session list or another device shows tells
//! the global feed (`droppen_drive::feed::global`): it asks for a prompt re-list, or it
//! pushes a frame of its own. Accounts, attention, prefs, screens and plugins do
//! this but sit below the feed, so they hold a [`Refresh`] and the feed
//! implements [`RefreshSink`]. Both inputs are fire-and-forget: with no client
//! attached they do nothing, and neither reports back.

use std::fmt::Display;
use std::sync::Arc;

/// What a [`Refresh`] sends into. The global feed implements it.
pub trait RefreshSink: Send + Sync {
    /// Ask for a prompt re-list, optionally naming a session (its source and
    /// id) the caller has just created and expects to appear.
    fn request_refresh(&self, expected: Option<(&dyn Display, &str)>);
    /// Push a frame that is not about the session list to every attached
    /// client.
    fn publish(&self, frame: String);
}

/// A cloneable handle on the global feed's inputs.
#[derive(Clone)]
pub struct Refresh(Arc<dyn RefreshSink>);

impl Refresh {
    /// A handle that sends into `sink`.
    pub fn new(sink: Arc<dyn RefreshSink>) -> Self {
        Self(sink)
    }

    /// See [`RefreshSink::request_refresh`].
    pub fn request_refresh(&self, expected: Option<(&dyn Display, &str)>) {
        self.0.request_refresh(expected);
    }

    /// See [`RefreshSink::publish`].
    pub fn publish(&self, frame: String) {
        self.0.publish(frame);
    }
}
