//! What the streams publish, as the sync task reads it: the session list's
//! rows, each session's transcript, and a resident session's draft. The
//! sessions sit above the agent, so the streams name none of their types:
//! [`crate::cloud`] implements these traits over them and the entry hands the
//! result to the link ([`crate::agent::link::LinkEnv::streams`]).

use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde_json::Value;

/// Where the sync task reads what it copies to the hub.
pub trait StreamSource: Send + Sync {
    /// The session list's rows: this server's own sessions, and with
    /// `external` also those it only found.
    fn sessions(&self, external: bool) -> BoxFuture<'_, Vec<Box<dyn ListedSession>>>;

    /// Whether `source` names a CLI whose sessions this server lists. A
    /// stream's row in `streams.db` keeps the name it was copied under, and
    /// one this build does not know is not looked for in the list.
    fn knows(&self, source: &str) -> bool;

    /// The draft of session `id` of `source` as it changes, when that session
    /// publishes one.
    fn drafts(&self, source: &str, id: &str) -> Option<Box<dyn DraftFeed>>;
}

/// One row of the session list, as the index carries it.
pub trait SessionRow: Send + Sync {
    /// The CLI the session runs under, as stream names and index keys spell it.
    fn source(&self) -> &str;
    /// The session's id.
    fn id(&self) -> &str;
    /// When it last changed, in ISO 8601.
    fn updated_at(&self) -> &str;
    /// Whether this server owns it, rather than only found it.
    fn owned(&self) -> bool;
    /// The row as the session list serves it, which every device reads in
    /// the index; `None` when it does not serialise.
    fn json(&self) -> Option<String>;

    /// The key a row is filed under in the index: `source/id`.
    fn row_key(&self) -> String {
        format!("{}/{}", self.source(), self.id())
    }
}

impl<T: SessionRow + ?Sized> SessionRow for Box<T> {
    fn source(&self) -> &str {
        (**self).source()
    }
    fn id(&self) -> &str {
        (**self).id()
    }
    fn updated_at(&self) -> &str {
        (**self).updated_at()
    }
    fn owned(&self) -> bool {
        (**self).owned()
    }
    fn json(&self) -> Option<String> {
        (**self).json()
    }
}

impl<T: SessionRow + ?Sized> SessionRow for &T {
    fn source(&self) -> &str {
        (**self).source()
    }
    fn id(&self) -> &str {
        (**self).id()
    }
    fn updated_at(&self) -> &str {
        (**self).updated_at()
    }
    fn owned(&self) -> bool {
        (**self).owned()
    }
    fn json(&self) -> Option<String> {
        (**self).json()
    }
}

/// A listed session, and its transcript.
pub trait ListedSession: SessionRow {
    /// Its transcript file's signature (`len:mtime`), `None` when the file
    /// cannot be read. A copy compares it to skip a session that did not change.
    fn signature(&self) -> BoxFuture<'_, Option<String>>;

    /// Its transcript at `sig`, through the parse cache the live feeds share.
    /// A read that fails answers the last good parse, or no items.
    fn transcript<'a>(&'a self, sig: &'a str) -> BoxFuture<'a, Arc<dyn Transcript>>;

    /// Its transcript read now, past that cache and without filling it, so a
    /// first copy of many sessions does not push out the parses the live
    /// feeds share; `None` when it cannot be read.
    fn read_transcript(&self) -> BoxFuture<'_, Option<Arc<dyn Transcript>>>;
}

/// One parse of a session's transcript: the items the live feed sends, and
/// the fingerprint of every prefix of them.
pub trait Transcript: Send + Sync {
    /// Its items.
    fn items(&self) -> &dyn Items;
    /// Its fingerprint chain.
    fn chain(&self) -> &dyn Fingerprints;
    /// The parse of its first `end` items, of the same file, so with the
    /// same chain up to `end`. `end` is at most [`Transcript::len`].
    fn prefix(&self, end: usize) -> Arc<dyn Transcript>;

    /// How many items it holds.
    fn len(&self) -> usize {
        self.items().len()
    }
}

/// A transcript's items, each as the live feed sends it: a copy holds what a
/// browser would have been sent live.
pub trait Items: Send + Sync {
    /// How many.
    fn len(&self) -> usize;
    /// Item `i` as JSON; `null` when it does not serialise.
    fn json(&self, i: usize) -> Value;
    /// The length in bytes of item `i` serialised; 0 when it does not serialise.
    fn size(&self, i: usize) -> usize;
    /// Item `i`'s timestamp as written, if it has one.
    fn timestamp(&self, i: usize) -> Option<&str>;
}

/// A transcript's fingerprint chain, as the live feed spells it: entry `i`
/// covers items `[0, i)`, so there is one more than there are items. A
/// reader resumes the live feed from a copy with these, so the copy uses the
/// feed's own spelling and resume overlap rather than one of its own.
pub trait Fingerprints: Send + Sync {
    /// How many entries.
    fn len(&self) -> usize;
    /// Entry `at`, `None` past the end.
    fn at(&self, at: usize) -> Option<String>;
    /// The resume marker of a window that ends at `end`: the entry as many
    /// items before it as a resume re-sends, `None` past the end.
    fn resume(&self, end: usize) -> Option<String>;
}

/// The draft of one session as it changes.
pub trait DraftFeed: Send {
    /// Wait for the next change; `false` once the session's channel is gone.
    fn changed(&mut self) -> BoxFuture<'_, bool>;
    /// The draft now, marked seen.
    fn seen(&mut self) -> Draft;
}

/// A draft as live readers are sent it.
pub struct Draft {
    /// Bumped on every change a page can see: a frame that says the same is
    /// not sent twice.
    pub v: u64,
    /// The draft as JSON.
    pub body: Value,
}
