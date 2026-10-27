//! The quota panel: what the `/usage` probes cost, and how often they run.

mod support;

use std::time::SystemTime;

use remote_hand_server::quota::USAGE_TTL;
use remote_hand_server::sessions::iso8601;
use serde_json::{json, Value};
use support::{call_count, plan_auth, plan_usage, Probe};

const USAGE: &str = concat!(
    r#"{"result": "Current session: 12% used · resets Aug 11, 7:30am (UTC)\n"#,
    r#"Current week (all models): 40% used"}"#
);

/// A stored, verified account whose probe directory answers `/usage`.
async fn probing_account(probe: &Probe, label: &str, secret: &str) -> String {
    let account = probe.create_account(label, secret).await;
    let id = account["id"].as_str().expect("id").to_string();
    plan_auth(&probe.data_dir().join("accounts").join(&id).join("cwd"), r#"{"loggedIn": true}"#);
    probe.send_ok("POST", &format!("/api/accounts/{id}/usage-verify"), json!({}), 200).await;
    plan_usage(&probe.data_dir().join("accounts").join(&id).join("claude"), USAGE);
    id
}

#[tokio::test]
async fn the_local_plan_and_windows_come_back_together() {
    let probe = Probe::start().await;
    // The plan label is the one thing read out of the user's own credentials.
    std::fs::create_dir_all(probe.local_root()).expect("local root");
    std::fs::write(
        probe.local_root().join(".credentials.json"),
        r#"{"claudeAiOauth": {"subscriptionType": "max", "rateLimitTier": "default_max_20x", "accessToken": "sk-ant-oat01-NEVERREAD"}}"#,
    )
    .expect("credentials");
    plan_usage(&probe.home, USAGE);

    let quota = probe.get_json("/api/stats/quota").await;
    assert_eq!(quota["claude"]["plan"], "Max 20x");
    assert_eq!(quota["claude"]["tier"], "default_max_20x");
    let windows = quota["claude"]["windows"].as_array().expect("windows");
    assert_eq!(windows.len(), 2, "{:?}", quota["claude"]);
    assert_eq!(windows[0]["label"], "5h");
    assert_eq!(windows[0]["usedPercent"], 12.0);
    assert_eq!(windows[0]["remainingPercent"], 88.0);
    assert_eq!(windows[1]["label"], "weekly");
    assert_eq!(quota["codex"], Value::Null, "no codex sessions, nothing to report");
    assert_eq!(quota["accounts"].as_array().expect("accounts").len(), 0);

    // Nothing out of the credentials file beyond the plan may ever be read.
    let raw = probe.get("/api/stats/quota").await.1;
    assert!(!raw.contains("NEVERREAD"), "the access token reached a response: {raw}");

    probe.stop().await;
}

#[tokio::test]
async fn a_config_dir_that_reports_nothing_is_not_logged_in() {
    let probe = Probe::start().await;
    let id = probing_account(&probe, "Silent", "sk-ant-oat01-SILENT01234567").await;
    // Verified, but the probe directory prints no windows.
    std::fs::remove_file(probe.data_dir().join("accounts").join(&id).join("claude/usage.out")).expect("remove");

    let quota = probe.get_json("/api/stats/quota").await;
    let account = &quota["accounts"][0];
    assert_eq!(account["usageEnabled"], true);
    assert_eq!(account["error"], "not_logged_in", "the CLI ran and said nothing about windows");
    assert_eq!(account["windows"].as_array().expect("windows").len(), 0);
    assert_eq!(account["updatedAt"], Value::Null);

    probe.stop().await;
}

#[tokio::test]
async fn an_account_that_never_opted_in_is_never_probed() {
    let probe = Probe::start().await;
    let account = probe.create_account("Unverified", "sk-ant-oat01-UNVERIFIED0123").await;
    let id = account["id"].as_str().expect("id");
    let config_dir = probe.data_dir().join("accounts").join(id).join("claude");
    plan_usage(&config_dir, USAGE);

    let quota = probe.get_json("/api/stats/quota").await;
    let reported = &quota["accounts"][0];
    assert_eq!(reported["usageEnabled"], false);
    assert_eq!(reported["windows"].as_array().expect("windows").len(), 0);
    assert_eq!(reported["error"], Value::Null, "not probed at all is not an error to explain away");
    assert_eq!(call_count(&config_dir), 0, "an account that never opted in costs no CLI startup");

    probe.stop().await;
}

#[tokio::test]
async fn readings_are_cached_per_config_dir_and_probes_never_overlap() {
    let probe = Probe::start().await;
    let first = probing_account(&probe, "One", "sk-ant-oat01-ONE0123456789").await;
    let second = probing_account(&probe, "Two", "sk-ant-oat01-TWO0123456789").await;
    plan_usage(&probe.home, USAGE);

    let quota = probe.get_json("/api/stats/quota").await;
    let accounts = quota["accounts"].as_array().expect("accounts");
    assert_eq!(accounts.len(), 2);
    for account in accounts {
        assert_eq!(account["windows"].as_array().expect("windows").len(), 2, "{account}");
        assert_eq!(account["error"], Value::Null);
        assert!(account["updatedAt"].as_str().is_some(), "a reading that produced windows is stamped");
    }

    let dirs = [
        probe.home.clone(),
        probe.data_dir().join("accounts").join(&first).join("claude"),
        probe.data_dir().join("accounts").join(&second).join("claude"),
    ];
    for dir in &dirs {
        assert_eq!(call_count(dir), 1, "one probe each so far: {}", dir.display());
    }

    // The fake CLI records an overlap whenever two probes run at once; the
    // 300 ms each one sleeps is what makes an overlap observable at all.
    assert!(
        !probe.home.join("overlap.txt").exists(),
        "probes must be serialized so N accounts never cost N CLI startups at once"
    );

    // Inside the TTL a second call re-reads the cache instead of the CLI.
    probe.get_json("/api/stats/quota").await;
    for dir in &dirs {
        assert_eq!(call_count(dir), 1, "a cached reading must not re-probe: {}", dir.display());
    }

    probe.stop().await;
}

/// The poll budget, asserted against the endpoint rather than the constant.
///
/// 429 is keyed by account, so an eager panel spends the allowance the user's
/// real work needs; the ceiling adopted here is 20 requests per hour per
/// account. Two things hold it and both are exercised below: the 180 s cache
/// TTL (3600 / 180 = 20), and the in-flight map that collapses concurrent
/// callers onto one probe. A caller cannot buy extra probes by asking harder,
/// whether it asks all at once or one after another.
#[tokio::test]
async fn hammering_the_quota_panel_cannot_buy_more_than_twenty_probes_an_hour() {
    let probe = Probe::start().await;
    let account = probing_account(&probe, "Hammered", "sk-ant-oat01-HAMMER0123456").await;
    plan_usage(&probe.home, USAGE);

    let dirs = [probe.home.clone(), probe.data_dir().join("accounts").join(&account).join("claude")];

    // Cold cache, sixteen callers at once: single-flight has to collapse them.
    // Serially these would be sixteen CLI startups, and an hour of them at this
    // rate would be far past the ceiling.
    let burst = (0..16).map(|_| probe.get_json("/api/stats/quota"));
    let answers = futures_util::future::join_all(burst).await;
    assert_eq!(answers.len(), 16);

    // Then twenty more, one after another, all inside the same TTL.
    for _ in 0..20 {
        probe.get_json("/api/stats/quota").await;
    }

    for dir in &dirs {
        let calls = call_count(dir);
        assert_eq!(
            calls,
            1,
            "36 polls inside one TTL cost exactly one probe; got {calls} for {}",
            dir.display()
        );
        assert!(
            calls as u64 <= 3600 / USAGE_TTL.as_secs(),
            "and one TTL's worth of probes can never exceed the hourly budget"
        );
    }
    assert_eq!(3600 / USAGE_TTL.as_secs(), 20, "the budget is the TTL: 3600 s / 180 s");

    probe.stop().await;
}

/// A rate-limited probe must not be dressed up as a logged-out account.
///
/// `not_logged_in` is a verdict the UI acts on by sending the user back through
/// `claude auth login` — which for an account that is merely over its limit is
/// both useless and alarming, and costs another round of requests to an account
/// that has none to spare.
#[tokio::test]
async fn a_rate_limited_probe_is_never_mistaken_for_a_logged_out_account() {
    let probe = Probe::start().await;
    let id = probing_account(&probe, "Limited", "sk-ant-oat01-LIMITED012345").await;
    let config_dir = probe.data_dir().join("accounts").join(&id).join("claude");
    // What the CLI prints instead of windows when the account is over its limit.
    plan_usage(&config_dir, r#"{"result": "API Error: 429 Too Many Requests"}"#);

    let quota = probe.get_json("/api/stats/quota").await;
    let account = &quota["accounts"][0];
    assert_eq!(
        account["error"], "probe_failed",
        "a 429 says the reading did not happen, not that the login is gone: {account}"
    );
    assert_eq!(account["windows"].as_array().expect("windows").len(), 0);

    // And the backed-off account is not re-probed by the next poll either.
    probe.get_json("/api/stats/quota").await;
    assert_eq!(call_count(&config_dir), 1, "a backed-off account is left alone");

    probe.stop().await;
}

#[tokio::test]
async fn codex_windows_fall_back_to_the_newest_recent_session_file() {
    let probe = Probe::start().await;
    // The fake `codex app-server` answers `account/rateLimits/read` with an
    // empty result, so this exercises the fallback — which is the only path a
    // machine without a working Codex login has.
    let sessions = probe.home.join(".codex/sessions/2024/08/01");
    std::fs::create_dir_all(&sessions).expect("codex sessions");
    let reading = |stamp: &str, used: f64| {
        format!(
            "{}\n",
            json!({
                "type": "event_msg",
                "timestamp": stamp,
                "payload": {
                    "type": "token_count",
                    "rate_limits": {
                        "plan_type": "pro",
                        "primary": {"used_percent": used, "window_minutes": 300, "resets_at": 1_722_470_400},
                        "secondary": {"used_percent": 60.0, "window_minutes": 10080},
                    },
                },
            })
        )
    };
    // Written first, so it is also the *older* file by mtime: the fallback
    // picks the newest reading, not the newest file.
    std::fs::write(sessions.join("rollout.jsonl"), reading(&iso8601(SystemTime::now()), 25.0)).expect("rollout");
    // Older than the fallback's cutoff: it must not answer for today.
    std::fs::write(sessions.join("ancient.jsonl"), reading("2024-08-01T00:00:00.000Z", 90.0)).expect("ancient rollout");

    let quota = probe.get_json("/api/stats/quota").await;
    assert_eq!(quota["codex"]["plan"], "pro");
    assert_eq!(
        quota["codex"]["source"], "session_log",
        "the app server did not answer, and the panel is told so"
    );
    assert_eq!(quota["codex"]["blocked"], Value::Null, "the logs never say whether usage is blocked");
    let windows = quota["codex"]["windows"].as_array().expect("windows");
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0]["label"], "5h");
    assert_eq!(windows[0]["remainingPercent"], 75.0, "the recent reading, not the ancient one");
    assert_eq!(windows[0]["resetsAt"], "2024-08-01T00:00:00.000Z");
    assert_eq!(windows[1]["label"], "weekly");
    assert_eq!(windows[1]["resetsAt"], Value::Null);

    probe.stop().await;
}
