//! What this server knows about the machine's own login, for each CLI.
//!
//! The *local login* is the one a session gets with no account: the user's own
//! `~/.claude` (or `~/.codex`), signed in — or not — by the user in a terminal.
//! It has no row in `accounts`, so nothing used to describe it, and the
//! new-session picker offered it as the default whatever state it was in. A
//! login whose OAuth session had expired weeks ago was offered exactly like a
//! working one, and every session started on it failed its first turn.
//!
//! The server already had the evidence; it just never put it in one place:
//!
//! * **a refused credential.** A turn on the local login that failed on its
//!   credential rests [`LOCAL_CLAUDE`](super::rest::LOCAL_CLAUDE) with class
//!   `auth`, indefinitely ([`super::rest`]);
//! * **the `/usage` fallback probe**, which runs against the local login too and
//!   files `not_logged_in` when the CLI answers with nothing
//!   ([`super::quota::readings`]);
//! * **a reading.** Every window a session or a probe read for the local login
//!   was read by a login that worked at that instant.
//!
//! [`claude_state`] weighs them by *when*: the newest piece of evidence decides.
//! That is what lets the answer heal on its own — a user who signs in again and
//! runs one turn on the local login produces a reading newer than the rest that
//! condemned it — without anything having to clear the rest first.
//!
//! Codex has only the first of the three here (its readings are a different
//! CLI's protocol and say nothing about a login), so its state is `signed_out`
//! on a refused credential and `unknown` otherwise.

use gotcha::Schematic;
use serde::{Deserialize, Serialize};

use super::quota::readings::SharedReading;
use super::quota::QuotaProbeError;
use super::rest::{AccountRest, LimitLog, LOCAL_CLAUDE, LOCAL_CODEX};
use crate::sessions::account::LimitClass;

/// Whether the machine's own login can start a session right now, as far as
/// this server can tell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "snake_case")]
pub enum LocalLoginState {
    /// The newest evidence is a reading the login made: it worked then.
    SignedIn,
    /// The newest evidence is a refusal: a turn failed on the credential, or the
    /// `/usage` probe found nobody signed in. Sessions on it will fail until the
    /// user signs in again (`claude`, then `/login`, in a terminal).
    SignedOut,
    /// Nothing either way yet.
    #[default]
    Unknown,
}

/// The machine's own login for one CLI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct LocalLogin {
    /// Whether it can start a session, as far as this server can tell.
    pub state: LocalLoginState,
    /// Its active rests, in the shape `Account.rest` uses — a refused credential
    /// as much as a quota window it ran out of.
    pub rest: Vec<AccountRest>,
    /// Somebody made it this CLI's default (`PUT /api/accounts/default` with
    /// `null`, the local-login radio in Settings). A start that names no account
    /// then runs on it whatever its state: a choice, not the absence of one, so
    /// nothing takes its place when it is signed out.
    #[serde(default)]
    pub is_default: bool,
}

/// Both CLIs' local logins, as `AccountList.local` reports them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct LocalLogins {
    /// The user's own `~/.claude`.
    pub claude: LocalLogin,
    /// The user's own `~/.codex`.
    pub codex: LocalLogin,
}

/// The claude local login's state from its evidence, newest first.
///
/// `auth_rest_at` is when an active `auth` rest on the local login was seen;
/// `reading` is the shared `/usage` reading file for it. Ties go to the
/// refusal, the direction that costs a click rather than a failed session.
pub fn claude_state(auth_rest_at: Option<i64>, reading: &SharedReading) -> LocalLoginState {
    let refused_by_probe = match (reading.probe_error, reading.probe_error_at_ms) {
        // `probe_failed` only says the reading did not happen.
        (Some(QuotaProbeError::NotLoggedIn), Some(at)) => Some(at),
        _ => None,
    };
    let refused = auth_rest_at.max(refused_by_probe);
    let worked = reading.newest().map(|window| window.read_at_ms);
    match (refused, worked) {
        (Some(refused), Some(worked)) if worked > refused => LocalLoginState::SignedIn,
        (Some(_), _) => LocalLoginState::SignedOut,
        (None, Some(_)) => LocalLoginState::SignedIn,
        (None, None) => LocalLoginState::Unknown,
    }
}

/// Both local logins, from this instance's limit log and the machine's shared
/// local reading. Neither is the default here: that is a setting of the store,
/// which [`super::store::AccountsStore::local_logins`] fills in.
pub fn local_logins(log: &LimitLog, local_reading: &SharedReading, now: i64) -> LocalLogins {
    let mut rests = log.active_rests(now);
    let claude_refused = log.active_observed_ms(LOCAL_CLAUDE, LimitClass::Auth, now);
    let codex_refused = log.active_observed_ms(LOCAL_CODEX, LimitClass::Auth, now);
    LocalLogins {
        claude: LocalLogin {
            state: claude_state(claude_refused, local_reading),
            rest: rests.remove(LOCAL_CLAUDE).unwrap_or_default(),
            is_default: false,
        },
        codex: LocalLogin {
            state: if codex_refused.is_some() {
                LocalLoginState::SignedOut
            } else {
                LocalLoginState::Unknown
            },
            rest: rests.remove(LOCAL_CODEX).unwrap_or_default(),
            is_default: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::limits::Verdict;
    use crate::accounts::quota::{QuotaReadingSource, QuotaWindow};
    use crate::accounts::rest::Observation;
    use droppen_instance::db::Db;
    use std::sync::Arc;

    const MINUTE: i64 = 60_000;

    fn window(label: &str) -> QuotaWindow {
        QuotaWindow {
            label: label.to_string(),
            used_percent: 10.0,
            remaining_percent: 90.0,
            resets_at: crate::openapi_ext::Nullable(None),
            window_minutes: 300,
            updated_at: crate::openapi_ext::Optional(None),
        }
    }

    fn read_at(at: i64) -> SharedReading {
        let mut reading = SharedReading::default();
        reading.merge(vec![window("5h")], at, QuotaReadingSource::Session);
        reading
    }

    fn not_logged_in_at(mut reading: SharedReading, at: i64) -> SharedReading {
        reading.probe_error = Some(QuotaProbeError::NotLoggedIn);
        reading.probe_error_at_ms = Some(at);
        reading
    }

    #[test]
    fn nothing_known_is_unknown_and_a_reading_is_signed_in() {
        assert_eq!(claude_state(None, &SharedReading::default()), LocalLoginState::Unknown);
        assert_eq!(claude_state(None, &read_at(5 * MINUTE)), LocalLoginState::SignedIn);
    }

    /// The session that started this: an OAuth session expired weeks ago, the
    /// probe said so, and a turn on it was refused. Nothing since has worked.
    #[test]
    fn a_refused_credential_or_an_empty_probe_is_signed_out() {
        let old_reading = read_at(MINUTE);
        assert_eq!(claude_state(Some(10 * MINUTE), &old_reading), LocalLoginState::SignedOut);
        assert_eq!(
            claude_state(None, &not_logged_in_at(old_reading.clone(), 10 * MINUTE)),
            LocalLoginState::SignedOut,
            "the probe's verdict alone is enough"
        );
        assert_eq!(claude_state(Some(10 * MINUTE), &SharedReading::default()), LocalLoginState::SignedOut);
    }

    /// Signing in again and running one turn is the whole fix: the reading that
    /// turn makes is newer than the rest, and nobody has to clear anything.
    #[test]
    fn a_reading_newer_than_the_refusal_heals_it() {
        assert_eq!(claude_state(Some(MINUTE), &read_at(10 * MINUTE)), LocalLoginState::SignedIn);
        assert_eq!(
            claude_state(None, &not_logged_in_at(read_at(10 * MINUTE), MINUTE)),
            LocalLoginState::SignedIn
        );
    }

    /// A probe that failed to run says nothing about the login.
    #[test]
    fn a_probe_that_failed_to_run_is_not_a_verdict() {
        let reading = SharedReading {
            probe_error: Some(QuotaProbeError::ProbeFailed),
            probe_error_at_ms: Some(MINUTE),
            ..Default::default()
        };
        assert_eq!(claude_state(None, &reading), LocalLoginState::Unknown);
    }

    #[test]
    fn the_local_rests_are_reported_with_the_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = LimitLog::new(Arc::new(Db::open(dir.path().join("state.db"))));
        let now = 1_000 * MINUTE;
        log.record(
            &Observation {
                account_id: LOCAL_CLAUDE.to_string(),
                session_id: Some("s1".into()),
                source: "claude:result",
                model: None,
                verdict: Verdict {
                    class: LimitClass::Auth,
                    scope: "all".into(),
                    until_ms: None,
                },
                raw: "Failed to authenticate".into(),
                reason: "Failed to authenticate".into(),
                may_rest: true,
            },
            now,
        )
        .expect("record");

        let logins = local_logins(&log, &read_at(now - MINUTE), now + MINUTE);
        assert_eq!(logins.claude.state, LocalLoginState::SignedOut);
        assert_eq!(logins.claude.rest.len(), 1);
        assert_eq!(logins.claude.rest[0].class, LimitClass::Auth);
        assert_eq!(logins.codex, LocalLogin::default(), "the other CLI's login is not touched");
    }
}
