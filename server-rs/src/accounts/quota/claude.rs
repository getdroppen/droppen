//! Claude's quota readings out of its own sessions: the windows a
//! `rate_limit_event` carries, and the login a session's reading is filed
//! under. The files the readings are kept in are the accounts'
//! ([`super::readings`]).

use serde_json::Value;

use super::{window, QuotaWindow, UsageLogin};
use crate::accounts::rest::iso_ms;
use droppen_instance::paths::ClaudeRoot;

/// Which login a session's reading belongs to.
///
/// * a session bound to an account bills that account, whichever root it is in;
/// * a session with no account in the user's own `~/.claude` bills the local
///   login;
/// * a session with no account in this instance's managed root runs on whatever
///   login that root itself holds — some other login, of nobody's choosing here —
///   and its reading is dropped rather than filed under the wrong name. So is one
///   whose root cannot be told.
pub fn session_login(account: Option<&str>, root: Option<ClaudeRoot>) -> Option<UsageLogin> {
    match (account, root) {
        (Some(id), _) => Some(UsageLogin::Account(id.to_string())),
        (None, Some(ClaudeRoot::Local)) => Some(UsageLogin::Local),
        (None, _) => None,
    }
}

/// The label and length of one `unifiedWindows` key, named the way
/// [`super::parse_claude_usage`] names the probe's windows, so the two sources
/// merge by label.
///
/// * `five_hour` → `5h`, `seven_day` → `weekly`;
/// * `seven_day_<family>` → `weekly (<Family>)`, spelled as the CLI's own
///   `/usage` report spells it — which for Sonnet is `Current week (Sonnet
///   only)`, read out of the formatter in claude 2.1.281;
/// * `seven_day_overage_included` → `weekly (Fable)`. **Inferred** from one
///   sample, whose CLI text was "You've reached your Fable limit". The CLI
///   itself scopes the window to a list of models that need usage credits past
///   it, so should that list ever hold more than Fable, this label will be too
///   narrow;
/// * any other `seven_day_<x>` → `weekly (<x>)`, as it comes;
/// * anything else is not a window this knows the length of, and is skipped.
fn unified_window_label(key: &str) -> Option<(String, i64)> {
    const WEEK: i64 = 10080;
    match key {
        "five_hour" => Some(("5h".to_string(), 300)),
        "seven_day" => Some(("weekly".to_string(), WEEK)),
        "seven_day_overage_included" => Some(("weekly (Fable)".to_string(), WEEK)),
        _ => {
            let scope = key.strip_prefix("seven_day_").filter(|scope| !scope.is_empty())?;
            let scope = match scope {
                "opus" => "Opus",
                "sonnet" => "Sonnet only",
                "haiku" => "Haiku",
                "fable" => "Fable",
                other => other,
            };
            Some((format!("weekly ({scope})"), WEEK))
        }
    }
}

/// Reading the windows out of a `rate_limit_event`'s `rate_limit_info`.
pub trait RateLimitWindows {
    /// The windows in a `rate_limit_event`'s `rate_limit_info`, whatever its
    /// status.
    ///
    /// `utilization` is a fraction; the panel speaks percent to one decimal,
    /// as the probe's report does. `resetsAt` is epoch seconds. A window with
    /// no utilization is skipped rather than read as zero.
    fn windows_from_rate_limit_info(&self) -> Vec<QuotaWindow>;
}

impl RateLimitWindows for Value {
    fn windows_from_rate_limit_info(&self) -> Vec<QuotaWindow> {
    let Some(Value::Object(unified)) = self.get("unifiedWindows") else {
        return vec![];
    };
    let mut windows: Vec<(bool, QuotaWindow)> = Vec::new();
    for (key, raw) in unified {
        let Some((label, minutes)) = unified_window_label(key) else {
            tracing::debug!(window = %key, "quota: skipping a unifiedWindows entry of unknown length");
            continue;
        };
        let Some(utilization) = raw.get("utilization").and_then(Value::as_f64).filter(|u| u.is_finite()) else {
            continue;
        };
        let used = (utilization * 1000.0).round() / 10.0;
        let resets_at = raw
            .get("resetsAt")
            .and_then(Value::as_f64)
            .filter(|secs| secs.is_finite() && *secs > 0.0)
            .map(|secs| iso_ms((secs * 1000.0) as i64));
        // A family's own key outranks the inferred one if both ever arrive in
        // one event: `seven_day_fable` would say what `overage_included` guesses.
        let inferred = key == "seven_day_overage_included";
        let candidate = window(label, minutes, used, resets_at);
        match windows.iter_mut().find(|(_, existing)| existing.label == candidate.label) {
            Some(slot) if slot.0 && !inferred => *slot = (inferred, candidate),
            Some(_) => {}
            None => windows.push((inferred, candidate)),
        }
    }
    windows.into_iter().map(|(_, window)| window).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `allowed` event, the kind claude writes before nearly every turn.
    const ALLOWED: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"isUsingOverage":false,"overageDisabledReason":"org_level_disabled","overageStatus":"rejected","rateLimitType":"five_hour","resetsAt":1790436000,"status":"allowed","unifiedWindows":{"five_hour":{"resetsAt":1790436000,"utilization":0.19},"seven_day":{"resetsAt":1790460000,"utilization":0.46}}}}"#;
    /// The `rate_limit_info` of a Fable weekly hit.
    const REJECTED_FABLE: &str = r#"{"isUsingOverage":false,"overageDisabledReason":"org_level_disabled","overageStatus":"rejected","rateLimitType":"seven_day_overage_included","resetsAt":1790575200,"status":"rejected","unifiedWindows":{"five_hour":{"resetsAt":1790436000,"utilization":0.02},"seven_day":{"resetsAt":1790575200,"utilization":0.82},"seven_day_overage_included":{"resetsAt":1790575200,"utilization":1}}}"#;

    #[test]
    fn an_allowed_event_is_a_reading_of_every_window_it_names() {
        let line: Value = serde_json::from_str(ALLOWED).expect("json");
        let windows = line["rate_limit_info"].windows_from_rate_limit_info();
        assert_eq!(windows.len(), 2);

        assert_eq!(windows[0].label, "5h");
        assert_eq!(windows[0].window_minutes, 300);
        assert_eq!(windows[0].used_percent, 19.0, "0.19 is 19 %, rounded to one decimal");
        assert_eq!(windows[0].remaining_percent, 81.0);
        assert_eq!(windows[0].resets_at.0.as_deref(), Some("2026-09-26T15:20:00.000Z"));

        assert_eq!(windows[1].label, "weekly");
        assert_eq!(windows[1].window_minutes, 10080);
        assert_eq!(windows[1].used_percent, 46.0);
        assert_eq!(windows[1].resets_at.0.as_deref(), Some("2026-09-26T22:00:00.000Z"));
    }

    #[test]
    fn the_fable_weekly_hit_maps_its_overage_window_to_weekly_fable() {
        let info: Value = serde_json::from_str(REJECTED_FABLE).expect("json");
        let windows = info.windows_from_rate_limit_info();
        let named: Vec<(&str, f64)> = windows.iter().map(|w| (w.label.as_str(), w.used_percent)).collect();
        assert_eq!(named, [("5h", 2.0), ("weekly", 82.0), ("weekly (Fable)", 100.0)]);
        assert_eq!(windows[2].remaining_percent, 0.0);
        assert_eq!(windows[2].window_minutes, 10080);
        assert_eq!(windows[2].resets_at.0.as_deref(), Some("2026-09-28T06:00:00.000Z"));
    }

    #[test]
    fn window_keys_are_named_the_way_the_probe_names_them() {
        let label = |key: &str| unified_window_label(key).map(|(label, _)| label);
        assert_eq!(label("seven_day_opus").as_deref(), Some("weekly (Opus)"));
        // `/usage` prints `Current week (Sonnet only)` for this window.
        assert_eq!(label("seven_day_sonnet").as_deref(), Some("weekly (Sonnet only)"));
        assert_eq!(label("seven_day_haiku").as_deref(), Some("weekly (Haiku)"));
        assert_eq!(label("seven_day_fable").as_deref(), Some("weekly (Fable)"));
        assert_eq!(
            label("seven_day_cowork").as_deref(),
            Some("weekly (cowork)"),
            "an unknown scope comes as it is"
        );
        assert_eq!(label("overage"), None, "no length to claim, so no window");
        assert_eq!(label("seven_day_"), None);

        // Both Fable keys in one event: the family's own name wins.
        let info = serde_json::json!({"unifiedWindows": {
            "seven_day_fable": {"utilization": 0.5},
            "seven_day_overage_included": {"utilization": 0.9},
        }});
        let windows = info.windows_from_rate_limit_info();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].used_percent, 50.0);

        // No `unifiedWindows`, or a window with no utilization: nothing to read.
        assert!(serde_json::json!({"status": "allowed"}).windows_from_rate_limit_info().is_empty());
        let partial = serde_json::json!({"unifiedWindows": {"five_hour": {"resetsAt": 1790436000}}});
        assert!(partial.windows_from_rate_limit_info().is_empty());
    }
    #[test]
    fn a_session_reading_goes_to_the_login_the_session_bills() {
        let id = "0b6f7a7e-2f7a-4d8e-9d0c-1a2b3c4d5e6f";
        assert_eq!(session_login(Some(id), Some(ClaudeRoot::Managed)), Some(UsageLogin::Account(id.into())));
        assert_eq!(
            session_login(Some(id), None),
            Some(UsageLogin::Account(id.into())),
            "the account decides, not the root"
        );
        assert_eq!(
            session_login(None, Some(ClaudeRoot::Local)),
            Some(UsageLogin::Local),
            "the user's own ~/.claude"
        );
        assert_eq!(
            session_login(None, Some(ClaudeRoot::Managed)),
            None,
            "the managed root's own login is nobody's here"
        );
        assert_eq!(session_login(None, None), None, "a root that cannot be told is skipped");
    }
}
