//! Claude's quota readings: the windows a `rate_limit_event` carries, the
//! login a session's reading is filed under, and the `/usage` probe that
//! reads a login nothing has run under lately. The files the readings are kept
//! in are the accounts' ([`super::readings`]).

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use super::{is_rate_limited, window, ProbeRoots, QuotaProbeError, QuotaWindow, UsageLogin, UsageProbe};
use crate::accounts::rest::iso_ms;
use crate::sessions::util::try_parse;
use droppen_base::process_env::{apply_env, claude_env};
use droppen_base::time::{iso8601, now_ms};
use droppen_instance::paths::ClaudeRoot;

/// `/usage` is a local slash command: it queries the usage endpoint directly, so
/// the run costs no tokens and takes ~1.5 s. `--no-session-persistence` keeps the
/// probe out of the session list.
const USAGE_ARGS: [&str; 5] = ["-p", "/usage", "--output-format", "json", "--no-session-persistence"];
const USAGE_TIMEOUT: Duration = Duration::from_secs(20);
/// The most of a probe's stdout, or stderr, that is judged; `output()` has
/// already read both in full.
const MAX_PROBE_OUTPUT: usize = 1024 * 1024;

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
/// [`parse_claude_usage`] names the probe's windows, so the two sources
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

// ------------------------------------------------------------- the /usage probe

/// The 5 h session window and the weekly ones, named the way codex windows are.
fn claude_window_label(raw: &str) -> (String, i64) {
    static WEEK: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let week = WEEK.get_or_init(|| regex::Regex::new(r"(?i)^current week\s*\((.+)\)$").expect("valid week pattern"));
    let name = raw.trim();
    if let Some(caps) = week.captures(name) {
        let scope = caps[1].trim();
        let label = if scope.eq_ignore_ascii_case("all models") {
            "weekly".to_string()
        } else {
            format!("weekly ({scope})")
        };
        return (label, 10080);
    }
    if name.eq_ignore_ascii_case("current session") {
        return ("5h".to_string(), 300);
    }
    (name.to_string(), 0)
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// The reset label usually carries no year, so pick the one that puts it in the
/// future — but take the year the CLI printed when it printed one.
///
/// The CLI formats these with `Intl` and adds `year: "numeric"` only when the
/// reset falls in a different calendar year from *now*, so the year-bearing form
/// (`Jan 2, 2027 at 12am (UTC)`) shows up exactly at the turn of the year — the
/// one moment when guessing is hardest and being told is most useful.
///
/// The CLI separates date and time two ways: a comma (`Aug 11, 7:30am (UTC)`)
/// and ` at ` (`Sep 10 at 11:59am (UTC)`). Both are accepted, because the
/// separator is the CLI's presentation choice and a reading that silently loses
/// its reset instant is the one failure mode this parser must not have — a
/// cached reading from either era still resolves.
fn parse_resets_at(raw: Option<&str>, now_ms: i64) -> Option<String> {
    static RESET: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RESET.get_or_init(|| {
        regex::Regex::new(r"(?i)^([A-Za-z]{3})\s+(\d{1,2})(?:,\s*(\d{4}))?(?:,\s*|\s+at\s+)(\d{1,2})(?::(\d{2}))?\s*(am|pm)\s*\(UTC\)$")
            .expect("valid reset pattern")
    });
    let caps = re.captures(raw?.trim())?;
    let month = MONTHS.iter().position(|m| *m == caps[1].to_lowercase())? as i64;
    let day: i64 = caps[2].parse().ok()?;
    let stated_year: Option<i64> = caps.get(3).and_then(|m| m.as_str().parse().ok());
    let hour12: i64 = caps[4].parse::<i64>().ok()? % 12;
    let minute: i64 = caps.get(5).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
    let hour = if caps[6].eq_ignore_ascii_case("pm") { hour12 + 12 } else { hour12 };

    // The day is the 1st plus the label's offset, so a day past the month's end
    // keeps normalising. The time of day stays plain arithmetic for the same
    // reason: a label the regex accepts but a calendar would not is read anyway.
    let at = |year: i64| -> Option<i64> {
        let first = chrono::NaiveDate::from_ymd_opt(year as i32, (month + 1) as u32, 1)?;
        let date = if day >= 1 {
            first.checked_add_days(chrono::Days::new((day - 1) as u64))?
        } else {
            first.checked_sub_days(chrono::Days::new(1))?
        };
        let midnight = date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis();
        Some(midnight + hour * 3_600_000 + minute * 60_000)
    };
    let timestamp = match stated_year {
        Some(year) => at(year)?,
        None => {
            use chrono::Datelike;
            let year = i64::from(chrono::DateTime::from_timestamp_millis(now_ms)?.year());
            let timestamp = at(year)?;
            // A window never resets in the past; a day of slack absorbs the rounding.
            if timestamp < now_ms - 24 * 60 * 60 * 1000 {
                at(year + 1)?
            } else {
                timestamp
            }
        }
    };
    Some(iso8601(std::time::UNIX_EPOCH + Duration::from_millis(u64::try_from(timestamp).ok()?)))
}

/// `Current session: 42% used · resets Sep 10 at 11:59am (UTC)` and friends.
pub fn parse_claude_usage(text: &str, now_ms: i64) -> Vec<QuotaWindow> {
    static LINE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = LINE
        .get_or_init(|| regex::Regex::new(r"(?m)^(.+?):\s*(\d+(?:\.\d+)?)%\s*used(?:\s*·\s*resets\s+(.+?))?\s*$").expect("valid usage-line pattern"));
    re.captures_iter(text)
        .filter_map(|caps| {
            let (label, minutes) = claude_window_label(&caps[1]);
            let used: f64 = caps[2].parse().ok()?;
            let clause = caps.get(3).map(|m| m.as_str());
            let resets_at = parse_resets_at(clause, now_ms);
            // A window that printed a reset clause we could not read is the one
            // way this parser fails quietly: the panel just shows no reset time,
            // which reads as "this window has none". Say it, with the clause
            // itself, so the next time the CLI reformats it costs one log line
            // rather than an investigation. One line per unparsed window, and a
            // probe runs every 180 s at the very most, so there is nothing to
            // rate-limit.
            if let (Some(clause), None) = (clause, &resets_at) {
                tracing::warn!(window = %label, clause = %clause, "quota: unrecognised reset label from `claude /usage`");
            }
            Some(window(label, minutes, used, resets_at))
        })
        .collect()
}

impl ProbeRoots {
    /// Run `/usage` against one config root.
    ///
    /// `config_dir` `None` means the user's own login (whatever the CLI picks
    /// up by itself); anything else is an account's probe directory. `TZ` is
    /// pinned so the reset labels always carry `(UTC)`, which is the only form
    /// [`parse_resets_at`] understands.
    pub async fn claude_usage(&self, config_dir: Option<&Path>) -> UsageProbe {
    let mut command = tokio::process::Command::new("claude");
    command
        .args(USAGE_ARGS)
        .current_dir(config_dir.map(Path::to_path_buf).unwrap_or_else(|| self.home.clone()))
        .stdin(std::process::Stdio::null())
        // A timed-out probe that kept running would also break the one-at-a-time
        // rule: the next queued probe would start beside it.
        .kill_on_drop(true);

    let env = match config_dir {
        Some(dir) => claude_env([
            ("CLAUDE_CONFIG_DIR".to_string(), dir.to_string_lossy().into_owned()),
            ("TZ".to_string(), "UTC".to_string()),
        ]),
        None => claude_env([("TZ", "UTC")]),
    };
    apply_env(&mut command, &env);

    // Non-zero exit, timeout, or no CLI at all: the login may be perfectly fine,
    // so this must not read as "you need to log in again". A timeout has no
    // output to read, so it cannot be a 429 as far as this can tell.
    let Ok(Ok(output)) = tokio::time::timeout(USAGE_TIMEOUT, command.output()).await else {
        return UsageProbe {
            windows: vec![],
            error: Some(QuotaProbeError::ProbeFailed),
            rate_limited: false,
        };
    };

    let stdout = String::from_utf8_lossy(&output.stdout[..output.stdout.len().min(MAX_PROBE_OUTPUT)]).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(MAX_PROBE_OUTPUT)]).into_owned();
    let rate_limited = is_rate_limited(&stdout) || is_rate_limited(&stderr);

    if !output.status.success() {
        return UsageProbe {
            windows: vec![],
            error: Some(QuotaProbeError::ProbeFailed),
            rate_limited,
        };
    }

    // Fall back to raw stdout if the CLI ever stops wrapping the reply in JSON.
    let text = try_parse(stdout.trim())
        .and_then(|payload| payload.get("result").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| stdout.clone());
    let windows = parse_claude_usage(&text, now_ms());

    if !windows.is_empty() {
        // A reading is a reading: whatever else was printed alongside it, this
        // config dir answered, so there is nothing to back off from.
        return UsageProbe {
            windows,
            error: None,
            rate_limited: false,
        };
    }
        UsageProbe {
            windows,
            // A 429 says the reading did not happen; only silence with no such
            // marker says there is nobody logged in. Getting this the wrong way
            // round would send a perfectly good account back through `auth
            // login`.
            error: Some(if rate_limited {
                QuotaProbeError::ProbeFailed
            } else {
                QuotaProbeError::NotLoggedIn
            }),
            rate_limited,
        }
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

    /// `2024-08-01T00:00:00Z`, so the reset-year rule has something to bite on.
    const NOW_MS: i64 = 1_722_470_400_000;

    #[test]
    fn usage_lines_become_named_windows() {
        let text = concat!(
            "Current session: 12% used · resets Aug 11, 7:30am (UTC)\n",
            "Current week (all models): 40% used · resets Aug 5, 3pm (UTC)\n",
            "Current week (Opus): 99.5% used\n",
            "not a usage line at all\n"
        );
        let windows = parse_claude_usage(text, NOW_MS);
        assert_eq!(windows.len(), 3);

        assert_eq!(windows[0].label, "5h");
        assert_eq!(windows[0].window_minutes, 300);
        assert_eq!(windows[0].used_percent, 12.0);
        assert_eq!(windows[0].remaining_percent, 88.0);
        assert_eq!(windows[0].resets_at.as_ref().map(String::as_str), Some("2024-08-11T07:30:00.000Z"));

        assert_eq!(windows[1].label, "weekly", "\"all models\" is the plain weekly window");
        assert_eq!(windows[1].window_minutes, 10080);
        assert_eq!(windows[1].resets_at.as_ref().map(String::as_str), Some("2024-08-05T15:00:00.000Z"));

        assert_eq!(windows[2].label, "weekly (Opus)");
        assert_eq!(windows[2].used_percent, 99.5);
        assert_eq!(windows[2].resets_at.as_ref(), None, "no reset clause, no reset time");
    }

    #[test]
    fn a_reset_that_would_land_in_the_past_rolls_to_next_year() {
        // Late December, read on 1 August: the label means *this* year.
        assert_eq!(
            parse_resets_at(Some("Dec 31, 11pm (UTC)"), NOW_MS).as_deref(),
            Some("2024-12-31T23:00:00.000Z")
        );
        // January, read on 1 August: more than a day in the past, so next year.
        assert_eq!(
            parse_resets_at(Some("Jan 2, 12am (UTC)"), NOW_MS).as_deref(),
            Some("2025-01-02T00:00:00.000Z")
        );
        assert_eq!(parse_resets_at(Some("Aug 11, 7:30am (CEST)"), NOW_MS), None, "only UTC labels parse");
        assert_eq!(parse_resets_at(None, NOW_MS), None);
    }

    /// The CLI drops the minutes at the top of the hour and adds the year when
    /// the reset lands outside the current one, so both forms have to parse.
    #[test]
    fn a_reset_the_cli_qualified_with_a_year_is_taken_at_its_word() {
        // Guessing would have put this in 2024, which is precisely the case the
        // CLI prints the year for.
        assert_eq!(
            parse_resets_at(Some("Jan 2, 2025, 12am (UTC)"), NOW_MS).as_deref(),
            Some("2025-01-02T00:00:00.000Z")
        );
        // A stated year is used even when it is behind us: the CLI knows the
        // reset it is describing better than the roll-forward rule does.
        assert_eq!(
            parse_resets_at(Some("Dec 31, 2023, 11pm (UTC)"), NOW_MS).as_deref(),
            Some("2023-12-31T23:00:00.000Z")
        );
        // `minute: undefined` on the hour — the label carries no `:00`.
        assert_eq!(
            parse_resets_at(Some("Aug 11, 7am (UTC)"), NOW_MS).as_deref(),
            Some("2024-08-11T07:00:00.000Z")
        );
        assert_eq!(
            parse_resets_at(Some("Aug 11, 12pm (UTC)"), NOW_MS).as_deref(),
            Some("2024-08-11T12:00:00.000Z")
        );
    }

    /// `2026-09-10T00:00:00Z`: the fixed "now" for the CLI fixtures below, so
    /// their reset instants are absolute.
    const SEP_2026_MS: i64 = 1_788_998_400_000;

    /// The CLI moved the date/time separator from `,` to ` at `, which turns
    /// every Claude window's `resetsAt` into `null` if unhandled; Codex reports
    /// a numeric timestamp and is unaffected. Both separators are asserted
    /// here: ` at ` is what the CLI prints, the comma is what readings cached
    /// earlier still carry.
    #[test]
    fn the_at_separator_the_cli_prints_today_parses_like_the_comma_did() {
        assert_eq!(
            parse_resets_at(Some("Sep 10 at 11:59am (UTC)"), SEP_2026_MS).as_deref(),
            Some("2026-09-10T11:59:00.000Z")
        );
        assert_eq!(
            parse_resets_at(Some("Sep 12 at 9:59pm (UTC)"), SEP_2026_MS).as_deref(),
            Some("2026-09-12T21:59:00.000Z")
        );
        // On the hour, the CLI drops the minutes with this separator too.
        assert_eq!(
            parse_resets_at(Some("Sep 12 at 9pm (UTC)"), SEP_2026_MS).as_deref(),
            Some("2026-09-12T21:00:00.000Z")
        );
        // Year-bearing, in whichever separator the CLI pairs it with.
        assert_eq!(
            parse_resets_at(Some("Jan 2, 2027 at 12am (UTC)"), SEP_2026_MS).as_deref(),
            Some("2027-01-02T00:00:00.000Z")
        );
        assert_eq!(
            parse_resets_at(Some("Jan 2, 2027, 12am (UTC)"), SEP_2026_MS).as_deref(),
            Some("2027-01-02T00:00:00.000Z")
        );
        // The zone is still the gate: `TZ=UTC` on the probe is what earns it.
        assert_eq!(parse_resets_at(Some("Sep 10 at 11:59am (Asia/Shanghai)"), SEP_2026_MS), None);
        // ` at ` is a separator, not a filler word: no time still means no reset.
        assert_eq!(parse_resets_at(Some("Sep 10 at (UTC)"), SEP_2026_MS), None);
    }

    /// A real `claude -p /usage --output-format json` reading from `2026-09-10`,
    /// spawned exactly as [`ProbeRoots::claude_usage`] spawns it.
    #[test]
    fn a_live_usage_reading_keeps_its_reset_instants() {
        let text = concat!(
            "You are currently using your subscription to power your Claude Code usage\n",
            "\n",
            "Current session: 42% used \u{b7} resets Sep 10 at 11:59am (UTC)\n",
            "Current week (all models): 83% used \u{b7} resets Sep 12 at 9:59pm (UTC)\n",
            "Current week (Fable): 82% used \u{b7} resets Sep 12 at 9:59pm (UTC)",
        );
        let windows = parse_claude_usage(text, SEP_2026_MS);
        assert_eq!(windows.len(), 3);

        assert_eq!(windows[0].label, "5h");
        assert_eq!(windows[0].used_percent, 42.0);
        assert_eq!(
            windows[0].resets_at.as_ref().map(String::as_str),
            Some("2026-09-10T11:59:00.000Z"),
            "the bug this fixture exists for: every window came back with a null reset"
        );

        assert_eq!(windows[1].label, "weekly");
        assert_eq!(windows[1].resets_at.as_ref().map(String::as_str), Some("2026-09-12T21:59:00.000Z"));

        assert_eq!(windows[2].label, "weekly (Fable)");
        assert_eq!(windows[2].resets_at.as_ref().map(String::as_str), Some("2026-09-12T21:59:00.000Z"));
    }

    /// The shape `claude -p /usage` really prints on a logged-in Max plan.
    ///
    /// Captured from the CLI's own formatter: a lead-in sentence, one line per
    /// window with the utilization floored to an integer, a model-scoped weekly
    /// window, server-named per-model windows, and the "what's contributing"
    /// block. Only the window lines may become windows — the prose around them
    /// must not.
    #[test]
    fn the_current_cli_usage_report_parses_to_windows_and_nothing_else() {
        let text = concat!(
            "You are currently using your subscription to power your Claude Code usage\n",
            "Current session: 12% used \u{b7} resets Aug 11, 7:30am (UTC)\n",
            "Current week (all models): 40% used \u{b7} resets Aug 5, 3pm (UTC)\n",
            "Current week (Sonnet only): 8% used \u{b7} resets Aug 5, 3pm (UTC)\n",
            "Fable: 3% used \u{b7} resets Aug 5, 3pm (UTC)\n",
            "What's contributing to your limits usage?\n",
            "Approximate, based on local sessions on this machine \u{2014} does not include other devices or claude.ai. \
             Behaviors are independent characteristics, not a breakdown.\n",
            "\n",
            "Last 24h \u{b7} 120 requests \u{b7} 8 sessions\n",
            "  30% of your usage hit a >100k-token cache miss\n",
            "  Top skills: /dataviz 30%, /code-review 20%\n",
            "  Top MCP servers: linear 12%\n",
        );
        let windows = parse_claude_usage(text, NOW_MS);
        let labels: Vec<&str> = windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["5h", "weekly", "weekly (Sonnet only)", "Fable"]);
        assert_eq!(windows[2].window_minutes, 10080, "a model-scoped weekly window is still a week");
        assert_eq!(windows[3].window_minutes, 0, "a server-named window has no length we can claim to know");
        assert_eq!(windows[0].used_percent, 12.0);
        assert_eq!(windows[0].resets_at.as_ref().map(String::as_str), Some("2024-08-11T07:30:00.000Z"));
    }

    /// What a probe reads when the CLI has no usable login.
    ///
    /// `/usage` is one command with two answers: the window report when a
    /// subscription login is readable, and the session cost summary when it is
    /// not. The fallback exits 0 and says nothing about percentages, so the only
    /// thing that must never happen is a line of it being read as a window.
    #[test]
    fn the_cost_summary_fallback_yields_no_windows() {
        let text = concat!(
            "Total cost:            $0.0000\n",
            "Total duration (API):  0s\n",
            "Total duration (wall): 0s\n",
            "Total code changes:    0 lines added, 0 lines removed\n",
            "Usage:                 0 input, 0 output, 0 cache read, 0 cache write",
        );
        assert!(parse_claude_usage(text, NOW_MS).is_empty());
        assert!(!is_rate_limited(text), "an unauthenticated probe is not a rate-limited one");
    }
}
