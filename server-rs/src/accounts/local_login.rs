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
//! Claude Code's login ([`super::login::claude::ClaudeCodeLogin`]) weighs them
//! by *when*: the newest piece of evidence decides.
//! That is what lets the answer heal on its own — a user who signs in again and
//! runs one turn on the local login produces a reading newer than the rest that
//! condemned it — without anything having to clear the rest first.
//!
//! Codex has only the first of the three here (its readings are a different
//! CLI's protocol and say nothing about a login), so its state is `signed_out`
//! on a refused credential and `unknown` otherwise.

use gotcha::Schematic;
use serde::{Deserialize, Serialize};

use super::login::Logins;
use super::quota::readings::SharedReading;
use super::rest::{AccountRest, LimitLog, LOCAL_CLAUDE, LOCAL_CODEX};
use super::store::AccountProvider;
use droppen_sessions::account::LimitClass;

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

impl LocalLogins {
    /// The local login of the CLI `provider` names.
    pub fn of(self, provider: AccountProvider) -> LocalLogin {
        match provider {
            AccountProvider::Claude => self.claude,
            AccountProvider::Codex => self.codex,
        }
    }
}

impl Logins {
    /// Both local logins, from this instance's limit log and the machine's shared
    /// local reading, each weighed by its own CLI's login. Neither is the default
    /// here: that is a setting of the store, which
    /// [`super::store::AccountsStore::local_logins`] fills in.
    pub fn local_logins(&self, log: &LimitLog, local_reading: &SharedReading, now: i64) -> LocalLogins {
        let mut rests = log.active_rests(now);
        let claude_refused = log.active_observed_ms(LOCAL_CLAUDE, LimitClass::Auth, now);
        let codex_refused = log.active_observed_ms(LOCAL_CODEX, LimitClass::Auth, now);
        LocalLogins {
            claude: LocalLogin {
                state: self.claude.local_state(claude_refused, local_reading),
                rest: rests.remove(LOCAL_CLAUDE).unwrap_or_default(),
                is_default: false,
            },
            codex: LocalLogin {
                state: self.codex.local_state(codex_refused, local_reading),
                rest: rests.remove(LOCAL_CODEX).unwrap_or_default(),
                is_default: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::limits::Verdict;
    use crate::accounts::quota::{QuotaProbeError, QuotaReadingSource, QuotaWindow};
    use crate::accounts::rest::Observation;
    use droppen_instance::db::Db;
    use std::sync::Arc;

    const MINUTE: i64 = 60_000;

    fn window(label: &str) -> QuotaWindow {
        QuotaWindow {
            label: label.to_string(),
            used_percent: 10.0,
            remaining_percent: 90.0,
            resets_at: droppen_http::openapi_ext::Nullable(None),
            window_minutes: 300,
            updated_at: droppen_http::openapi_ext::Optional(None),
        }
    }

    fn read_at(at: i64) -> SharedReading {
        let mut reading = SharedReading::default();
        reading.merge(vec![window("5h")], at, QuotaReadingSource::Session);
        reading
    }

    /// A turn on claude's local login refused on its credential at `at`.
    fn refuse_local_claude(log: &LimitLog, at: i64) {
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
            at,
        )
        .expect("record");
    }

    /// Each CLI's start reads its own local login, never the other's.
    #[test]
    fn each_cli_reads_its_own_local_login() {
        let logins = LocalLogins {
            claude: LocalLogin {
                state: LocalLoginState::SignedIn,
                ..Default::default()
            },
            codex: LocalLogin {
                state: LocalLoginState::SignedOut,
                ..Default::default()
            },
        };
        assert_eq!(logins.clone().of(AccountProvider::Claude).state, LocalLoginState::SignedIn);
        assert_eq!(logins.of(AccountProvider::Codex).state, LocalLoginState::SignedOut);
    }

    #[test]
    fn the_local_rests_are_reported_with_the_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = LimitLog::new(Arc::new(Db::open(dir.path().join("state.db"))));
        let now = 1_000 * MINUTE;
        refuse_local_claude(&log, now);

        let logins = crate::app::logins().local_logins(&log, &read_at(now - MINUTE), now + MINUTE);
        assert_eq!(logins.claude.state, LocalLoginState::SignedOut);
        assert_eq!(logins.claude.rest.len(), 1);
        assert_eq!(logins.claude.rest[0].class, LimitClass::Auth);
        assert_eq!(logins.codex, LocalLogin::default(), "the other CLI's login is not touched");
    }

    /// Each CLI's state is weighed by its own login: Claude Code's reads the
    /// reading, Codex's does not.
    #[test]
    fn each_local_login_is_weighed_by_its_own_clis_login() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = LimitLog::new(Arc::new(Db::open(dir.path().join("state.db"))));
        let logins = crate::app::logins();
        let now = 1_000 * MINUTE;

        let probed = SharedReading {
            probe_error: Some(QuotaProbeError::NotLoggedIn),
            probe_error_at_ms: Some(now - MINUTE),
            ..Default::default()
        };
        assert_eq!(
            logins.local_logins(&log, &probed, now).claude.state,
            LocalLoginState::SignedOut,
            "the probe finding nobody signed in is enough on its own"
        );

        refuse_local_claude(&log, now - 2 * MINUTE);
        let healed = logins.local_logins(&log, &read_at(now - MINUTE), now);
        assert_eq!(healed.claude.state, LocalLoginState::SignedIn, "a reading newer than the refusal");
        assert_eq!(healed.codex.state, LocalLoginState::Unknown, "claude's reading says nothing about codex's login");
    }
}
