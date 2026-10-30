//! The local logins, each weighed by its own CLI's login.

use std::sync::Arc;

use droppen_accounts::limits::Verdict;
use droppen_accounts::local_login::*;
use droppen_accounts::quota::readings::SharedReading;
use droppen_accounts::quota::{QuotaProbeError, QuotaReadingSource, QuotaWindow};
use droppen_accounts::rest::{LimitLog, Observation, LOCAL_CLAUDE};
use droppen_instance::db::Db;
use droppen_sessions::account::LimitClass;

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
