//! Single-use credentials for the two places a bearer header cannot go.
//!
//! Two browser APIs cannot carry an `Authorization` header: `WebSocket`, whose
//! constructor takes a URL and nothing else, and anything that hands a URL to
//! the browser to fetch on its own — `<img src>`, `<a download>`, an `<iframe>`.
//! Putting the *session* token in those URLs would write it into every proxy
//! log, every `Referer`, and the browser's own history.
//!
//! So a ticket: a second random string, minted by an already-authenticated
//! request, that is worth much less than the token it came from.
//!
//! * `scope=ws` — sixty seconds, and **consumed by the check**. It is spent
//!   between `new WebSocket(url)` and the handshake completing, so one use is
//!   all it ever needs; a copy scraped out of a log is already dead.
//! * `scope=download` — sixty seconds, reusable. A gallery of thumbnails is
//!   many requests for one intent, and a browser that retries a `<img>` must
//!   not get a broken image.
//!
//! Every ticket is bound to the login that minted it — the digest of its
//! session — and [`super::guard`] honours one only while that session is still
//! live and its account still on file. Logging out, revoking the session,
//! changing the password or removing the account kills the tickets with it.
//!
//! A ticket names its scope and only works on the paths that scope covers; see
//! [`super::guard`], which is the only thing that redeems one.
//!
//! In memory, never on disk: a ticket outlives neither a restart nor its own
//! minute, and writing it down would be a strictly worse copy of the token.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use gotcha::Schematic;
use rand::RngCore;
use serde::{Deserialize, Serialize};

/// How many random bytes a ticket carries — the same 256 bits a token does,
/// because the ticket is a bearer credential too, just a short-lived one.
const TICKET_BYTES: usize = 32;

/// How long a websocket ticket is good for.
pub const WS_TTL: Duration = Duration::from_secs(60);
/// How long a download ticket is good for.
pub const DOWNLOAD_TTL: Duration = Duration::from_secs(60);

/// Above this many live tickets, minting one sweeps the expired first and then
/// refuses. A ceiling rather than a cleanup task: the only way to reach it is a
/// client in a loop, and a bounded map is the difference between a bug and a way
/// to exhaust the server's memory.
const MAX_LIVE: usize = 4096;

/// What a ticket may be spent on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum TicketScope {
    /// The four websocket upgrades: `/api/events`, `/api/agg/ws`,
    /// `/api/sessions/{source}/{id}/ws` and `/api/terminals/{id}/ws`.
    Ws,
    /// `GET /api/fs/download`, and nothing else.
    Download,
}

impl TicketScope {
    /// How long a ticket in this scope lives.
    pub fn ttl(self) -> Duration {
        match self {
            Self::Ws => WS_TTL,
            Self::Download => DOWNLOAD_TTL,
        }
    }

    /// Whether redeeming a ticket in this scope destroys it.
    pub fn single_use(self) -> bool {
        matches!(self, Self::Ws)
    }
}

struct Entry {
    scope: TicketScope,
    username: String,
    /// The digest of the session that minted it.
    session: String,
    expires: Instant,
}

/// What a redeemed ticket speaks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redeemed {
    /// Who minted it.
    pub username: String,
    /// The digest of the session it was minted under, which has to be live
    /// still for the ticket to count.
    pub session: String,
}

/// Every live ticket this server has minted.
///
/// Lives in [`crate::drive::Drive`] rather than in a `static`, like every other
/// registry in this server: two servers in one process — which is what the
/// integration tests run — must not be able to redeem each other's tickets.
#[derive(Default)]
pub struct Tickets {
    entries: Mutex<HashMap<String, Entry>>,
}

impl std::fmt::Debug for Tickets {
    /// Hand-written so a `{:?}` of the application state can never print a live
    /// ticket. Only the count is anybody's business.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Tickets").field("live", &self.len()).finish()
    }
}

impl Tickets {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many tickets are on file; the expiry sweep is observable through it.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether no ticket is on file.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().expect("auth tickets poisoned")
    }

    /// Mint a ticket for `username`'s session `session` (its digest) in
    /// `scope`, or `None` if the table is full.
    pub fn issue(&self, username: &str, session: &str, scope: TicketScope) -> Option<(String, Instant)> {
        self.issue_at(username, session, scope, Instant::now())
    }

    fn issue_at(&self, username: &str, session: &str, scope: TicketScope, now: Instant) -> Option<(String, Instant)> {
        let ticket = new_ticket();
        let expires = now + scope.ttl();
        let mut entries = self.lock();
        entries.retain(|_, entry| entry.expires > now);
        if entries.len() >= MAX_LIVE {
            return None;
        }
        entries.insert(
            ticket.clone(),
            Entry {
                scope,
                username: username.to_string(),
                session: session.to_string(),
                expires,
            },
        );
        Some((ticket, expires))
    }

    /// Spend a ticket: who minted it, under which session, when it was live and
    /// in `scope`; a `ws` ticket is gone afterwards either way. Whether that
    /// session and account still stand is the caller's to check.
    pub fn redeem(&self, ticket: &str, scope: TicketScope) -> Option<Redeemed> {
        self.redeem_at(ticket, scope, Instant::now())
    }

    fn redeem_at(&self, ticket: &str, scope: TicketScope, now: Instant) -> Option<Redeemed> {
        let mut entries = self.lock();
        let entry = entries.get(ticket)?;
        // Wrong scope or too late: leave it where it is. Consuming it would let
        // anyone who can guess a path cancel somebody else's pending upgrade,
        // and it is going to expire on its own in under a minute regardless.
        if entry.scope != scope || entry.expires <= now {
            return None;
        }
        let redeemed = Redeemed {
            username: entry.username.clone(),
            session: entry.session.clone(),
        };
        if scope.single_use() {
            entries.remove(ticket);
        }
        Some(redeemed)
    }
}

/// A fresh ticket: 32 bytes from the OS, base64url without padding.
fn new_ticket() -> String {
    let mut bytes = [0u8; TICKET_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ws_ticket_works_exactly_once() {
        let tickets = Tickets::new();
        let (ticket, _) = tickets.issue("ada", "s-1", TicketScope::Ws).expect("issue");
        assert_eq!(tickets.redeem(&ticket, TicketScope::Ws).map(|r| r.username).as_deref(), Some("ada"));
        assert_eq!(tickets.redeem(&ticket, TicketScope::Ws), None, "spent");
        assert!(tickets.is_empty());
    }

    #[test]
    fn a_download_ticket_works_as_often_as_the_page_needs() {
        let tickets = Tickets::new();
        let (ticket, _) = tickets.issue("ada", "s-1", TicketScope::Download).expect("issue");
        for _ in 0..10 {
            assert_eq!(tickets.redeem(&ticket, TicketScope::Download).map(|r| r.username).as_deref(), Some("ada"));
        }
        assert_eq!(tickets.len(), 1);
    }

    /// The scopes are the point: a thumbnail URL that leaks out of a page must
    /// not be a way to open a shell.
    #[test]
    fn a_ticket_is_worthless_in_the_other_scope() {
        let tickets = Tickets::new();
        let (download, _) = tickets.issue("ada", "s-1", TicketScope::Download).expect("issue");
        let (ws, _) = tickets.issue("ada", "s-1", TicketScope::Ws).expect("issue");
        assert_eq!(tickets.redeem(&download, TicketScope::Ws), None);
        assert_eq!(tickets.redeem(&ws, TicketScope::Download), None);
        assert_eq!(
            tickets.redeem(&ws, TicketScope::Ws).map(|r| r.username).as_deref(),
            Some("ada"),
            "and is not consumed by the miss"
        );
    }

    #[test]
    fn a_ticket_stops_working_when_it_expires() {
        let tickets = Tickets::new();
        let start = Instant::now();
        let (ws, _) = tickets.issue_at("ada", "s-1", TicketScope::Ws, start).expect("issue");
        let (download, _) = tickets.issue_at("ada", "s-1", TicketScope::Download, start).expect("issue");

        let after_a_minute = start + WS_TTL + Duration::from_secs(1);
        assert_eq!(tickets.redeem_at(&ws, TicketScope::Ws, after_a_minute), None);
        let within = start + DOWNLOAD_TTL - Duration::from_secs(1);
        assert_eq!(
            tickets.redeem_at(&download, TicketScope::Download, within).map(|r| r.username).as_deref(),
            Some("ada"),
            "reusable while it lives"
        );
        assert_eq!(
            tickets.redeem_at(&download, TicketScope::Download, after_a_minute),
            None,
            "a minute at most"
        );
    }

    #[test]
    fn minting_sweeps_what_has_already_expired() {
        let tickets = Tickets::new();
        let start = Instant::now();
        for _ in 0..5 {
            tickets.issue_at("ada", "s-1", TicketScope::Ws, start).expect("issue");
        }
        assert_eq!(tickets.len(), 5);
        tickets
            .issue_at("ada", "s-1", TicketScope::Ws, start + WS_TTL + Duration::from_secs(1))
            .expect("issue");
        assert_eq!(tickets.len(), 1, "the dead five went with the sixth being minted");
    }

    #[test]
    fn the_table_is_bounded() {
        let tickets = Tickets::new();
        let start = Instant::now();
        for _ in 0..MAX_LIVE {
            assert!(tickets.issue_at("ada", "s-1", TicketScope::Download, start).is_some());
        }
        assert!(
            tickets.issue_at("ada", "s-1", TicketScope::Download, start).is_none(),
            "a client in a loop is not a memory leak"
        );
    }

    #[test]
    fn a_ticket_names_the_session_that_minted_it() {
        let tickets = Tickets::new();
        let (ticket, _) = tickets.issue("ada", "digest-of-the-login", TicketScope::Download).expect("issue");
        assert_eq!(
            tickets.redeem(&ticket, TicketScope::Download),
            Some(Redeemed {
                username: "ada".into(),
                session: "digest-of-the-login".into()
            })
        );
    }

    #[test]
    fn a_ticket_is_43_url_safe_characters() {
        let ticket = new_ticket();
        assert_eq!(ticket.len(), 43);
        assert!(ticket.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
    }
}
