//! Which CLI failures are limits, and how long an account should rest after one.
//!
//! Phase 1 of automatic account rotation: **detect.** Nothing in this module
//! retries a turn, switches an account or holds a queue. What it does is read
//! what the CLI said and answer three questions: is this a limit, which kind,
//! and when does it lift. [`super::rest`] writes the answer down, and
//! [`super::rotation`] decides what a session does about it.
//!
//! Everything here is pure — text or payload in, [`Verdict`] out — so the
//! wordings it was built from are pinned by tests rather than by an account
//! running out of quota on somebody's phone.
//!
//! # Where the wordings come from
//!
//! Real `result` lines from claude 2.1.266, read out of this server's own flight
//! recordings:
//!
//! ```text
//! is_error: true, subtype: success, api_error_status: 429
//!   You've hit your session limit · resets 2:30am (Asia/Tokyo)
//!   You've hit your weekly limit · resets Sep 20 at 7am (Asia/Tokyo)
//! is_error: true, subtype: success, api_error_status: 404
//!   There's an issue with the selected model (claude-fable-5-1).   <- not a limit
//! is_error: true, subtype: success, api_error_status: 400
//!   Prompt is too long · automatic compaction failed: …            <- not a limit
//! ```
//!
//! and the `rate_limit_event` line claude writes before every turn, which says
//! `status: allowed` almost always and `status: rejected` — with the window and
//! its epoch reset — when the turn is about to be refused. The signal list is
//! adapted from magpie's router, with its known mistake corrected: context-length
//! errors mention a "limit" too, so **exclusions are checked first**.
//!
//! # Scope
//!
//! Every limit covers the whole account (`all`) unless the CLI names a model
//! family. A weekly limit is `all` too: in the recordings, the plain "You've
//! hit your weekly limit" arrives beside a `rate_limit_event` whose
//! `rateLimitType` is `seven_day` — the window every model shares. Only a
//! weekly limit that names its family — "Opus weekly limit", "You've reached
//! your Fable limit", `rateLimitType: seven_day_opus` — is scoped to `opus`,
//! `sonnet`, `fable` or `haiku`. The session's model is recorded beside the
//! event, but it never decides the scope: a family nobody named is a guess.
//!
//! # A weekly window that does not say whose it is
//!
//! A `rate_limit_event` names its window, and two of the names say what they
//! cover: `seven_day` is every model's, `seven_day_<family>` is one family's.
//! Any other `seven_day_*` does not — and the first real one proved it matters.
//! A Fable weekly hit arrived as `rateLimitType: seven_day_overage_included`
//! with the general `seven_day` window at 82 % and still usable for Opus; read
//! as `all`, it rested the whole account for three days over one exhausted
//! model. The `result` a moment later said which: "You've reached your Fable
//! limit", with no reset time in it.
//!
//! So such an event is a [`WindowHint`], not a rest: recorded with scope
//! [`SCOPE_UNKNOWN`], and held by the session's driver for the `result` that
//! follows within [`HINT_WINDOW`]. The result supplies the scope and the hint
//! the time — [`with_hint`]: a reset in the result's own words, then the hint's
//! `resetsAt`, then the class default. A result that names no family either
//! takes the whole account with the hint's time, which is what a plain weekly
//! limit always did. An event nobody follows rests nothing.
//!
//! # Time zones
//!
//! The CLI prints the reset as the *server's* local wall-clock time with the
//! IANA name in parentheses. There is deliberately no `chrono-tz` in this crate,
//! so the time is read against [`chrono::Local`] and trusted only when the name
//! in parentheses is the zone this process runs in; anything else leaves the
//! reset unparsed and the class default applies.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, TimeZone};
use gotcha::Schematic;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What kind of limit a failure was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "snake_case")]
pub enum LimitClass {
    /// The rolling five-hour window: "You've hit your session limit".
    Session,
    /// A weekly window, overall or for one model family.
    Weekly,
    /// A request-rate limit, or the vendor saying it is overloaded.
    Rate,
    /// Out of prepaid credit, or a billing problem.
    Credit,
    /// The credential itself was refused.
    Auth,
    /// Limit-shaped wording that nothing more specific matched.
    Other,
}

impl LimitClass {
    /// The name the tables and the API use.
    pub fn as_str(self) -> &'static str {
        match self {
            LimitClass::Session => "session",
            LimitClass::Weekly => "weekly",
            LimitClass::Rate => "rate",
            LimitClass::Credit => "credit",
            LimitClass::Auth => "auth",
            LimitClass::Other => "other",
        }
    }

    /// The inverse of [`LimitClass::as_str`]; a row nobody recognises is `other`.
    pub fn parse(raw: &str) -> Self {
        match raw {
            "session" => LimitClass::Session,
            "weekly" => LimitClass::Weekly,
            "rate" => LimitClass::Rate,
            "credit" => LimitClass::Credit,
            "auth" => LimitClass::Auth,
            _ => LimitClass::Other,
        }
    }
}

/// The scope of every limit that is not a per-model weekly window.
pub const SCOPE_ALL: &str = "all";
/// The scope an event is logged under when its window does not say which
/// models it covers; such an event never rests an account on its own.
pub const SCOPE_UNKNOWN: &str = "unknown";
/// How long a [`WindowHint`] waits for the `result` it explains — the same
/// window the log uses to call two sightings one hit.
pub const HINT_WINDOW: Duration = Duration::from_secs(10);

/// How long an account rests when nothing said when the limit lifts.
pub const SESSION_REST: Duration = Duration::from_secs(30 * 60);
/// See [`SESSION_REST`].
pub const WEEKLY_REST: Duration = Duration::from_secs(60 * 60);
/// See [`SESSION_REST`].
pub const RATE_REST: Duration = Duration::from_secs(60);
/// See [`SESSION_REST`].
pub const CREDIT_REST: Duration = Duration::from_secs(30 * 60);
/// The first rest an unclassified limit earns, once it has repeated.
pub const OTHER_REST_FLOOR: Duration = Duration::from_secs(60);
/// The longest rest an unclassified limit earns, however often it repeats.
pub const OTHER_REST_CAP: Duration = Duration::from_secs(10 * 60);

/// How long a class of limit rests an account when nothing said for how long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rest {
    /// No rest at all: a one-off `other` is recorded and nothing more.
    Skip,
    /// Rest for this long from now.
    For(Duration),
    /// Rest until somebody clears it or the credential changes.
    Indefinite,
}

/// The default rest for `class`.
///
/// `streak` is how many `other` events for the same account came *immediately
/// before* this one, and only matters to `other`: the first is recorded without
/// a rest (one odd failure is not a pattern), and each repeat after it doubles
/// the rest from [`OTHER_REST_FLOOR`] up to [`OTHER_REST_CAP`].
pub fn default_rest(class: LimitClass, streak: u32) -> Rest {
    match class {
        LimitClass::Session => Rest::For(SESSION_REST),
        LimitClass::Weekly => Rest::For(WEEKLY_REST),
        LimitClass::Rate => Rest::For(RATE_REST),
        LimitClass::Credit => Rest::For(CREDIT_REST),
        LimitClass::Auth => Rest::Indefinite,
        LimitClass::Other if streak == 0 => Rest::Skip,
        LimitClass::Other => {
            // 1, 2, 4, 8 minutes, then the cap. The shift is bounded so a long
            // streak cannot overflow on its way to being capped.
            let factor = 1u32 << (streak - 1).min(8);
            Rest::For((OTHER_REST_FLOOR * factor).min(OTHER_REST_CAP))
        }
    }
}

/// What a failure says about the account it happened under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// Which kind of limit.
    pub class: LimitClass,
    /// `all`, or the model family a weekly window belongs to.
    pub scope: String,
    /// When the limit lifts, epoch milliseconds, if the text or payload said.
    pub until_ms: Option<i64>,
}

// ------------------------------------------------------------ the patterns

/// Every regex this module matches with, compiled once. All of them run over
/// lower-cased text.
struct Patterns {
    /// Context-length failures mention a "limit" too, and are nobody's quota.
    excluded: Regex,
    /// `Claude AI usage limit reached|1790157600` — the older CLI's marker.
    epoch: Regex,
    weekly: Regex,
    /// A weekly limit that names its model family, which is captured.
    named_weekly: Regex,
    session: Regex,
    auth: Regex,
    credit: Regex,
    rate: Regex,
    /// A bare word "limit", for the reset-time and catch-all rules below.
    limit: Regex,
    resets: Regex,
    /// The shape a success-labelled result must have to be read as a limit.
    definitive: Regex,
}

fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let re = |pattern: &str| Regex::new(pattern).expect("a valid limit pattern");
        Patterns {
            excluded: re(r"context|prompt is too long|max_tokens|token limit|input length"),
            epoch: re(r"limit reached\|(\d{10})\b"),
            weekly: re(
                r"weekly (?:usage )?limit|hit your weekly|\b(?:fable|opus|sonnet|haiku) (?:weekly )?limit|weekly (?:fable|opus|sonnet|haiku) limit",
            ),
            named_weekly: re(r"\b(fable|opus|sonnet|haiku) (?:weekly )?limit|\bweekly (fable|opus|sonnet|haiku) limit"),
            session: re(r"session limit|hit your [^\n·.]{0,40}?\blimit\b|usage limit"),
            auth: re(
                r"\b401\b|invalid bearer|oauth access token is invalid|oauth session expired|oauth token has expired|not logged in|authentication|invalid api key|invalid x-api-key|please run /login|\bunauthori[sz]ed\b",
            ),
            credit: re(r"insufficient|balance|credit|billing|payment required|\b402\b|欠费|余额"),
            rate: re(r"rate.?limit|too many requests|\b429\b|overloaded"),
            limit: re(r"\blimits?\b|quota"),
            resets: re(concat!(
                r"\bresets?\s+(?:at\s+|on\s+)?",
                r"(?:(?P<mon>jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?\s+(?P<day>\d{1,2})(?:st|nd|rd|th)?,?\s+(?:at\s+)?)?",
                r"(?P<h>\d{1,2})(?::(?P<m>\d{2}))?\s*(?P<ap>am|pm)?",
                r"(?:\s*\((?P<tz>[^)]+)\))?",
            )),
            definitive: re(
                r"^(?:api error:\s*|error:\s*)?(?:you(?:'|’)?ve|you have) (?:hit|reached) your [^\n]{0,40}?\blimit\b|limit reached\|\d{10}\b",
            ),
        }
    })
}

// ------------------------------------------------------------ text

/// Which class of limit `text` describes, or `None` for a failure that is not
/// one — including every context-length failure, whatever else it says.
///
/// `status` is the HTTP status the CLI reported beside the text
/// (`api_error_status` on a claude `result`), and only breaks a tie the words
/// could not: a 429 whose wording is new to this module is still a rate limit.
pub fn classify(text: &str, status: Option<u16>) -> Option<LimitClass> {
    let lower = text.to_lowercase();
    let p = patterns();
    if p.excluded.is_match(&lower) {
        return None;
    }
    if p.epoch.is_match(&lower) {
        return Some(if lower.contains("weekly") {
            LimitClass::Weekly
        } else {
            LimitClass::Session
        });
    }
    if p.weekly.is_match(&lower) {
        return Some(LimitClass::Weekly);
    }
    if p.session.is_match(&lower) {
        return Some(LimitClass::Session);
    }
    if p.auth.is_match(&lower) {
        return Some(LimitClass::Auth);
    }
    if p.credit.is_match(&lower) {
        return Some(LimitClass::Credit);
    }
    if p.rate.is_match(&lower) {
        return Some(LimitClass::Rate);
    }
    let limitish = p.limit.is_match(&lower);
    // "Limit reached · resets 5pm" names no window, but a limit with a reset
    // time is a usage window, and the five-hour one is by far the likeliest.
    if limitish && p.resets.is_match(&lower) {
        return Some(LimitClass::Session);
    }
    match status {
        Some(429) => Some(LimitClass::Rate),
        Some(401) => Some(LimitClass::Auth),
        Some(402) => Some(LimitClass::Credit),
        _ if limitish => Some(LimitClass::Other),
        _ => None,
    }
}

/// The scope a limit of `class` covers.
///
/// The whole account, unless this is a weekly limit whose words name a model
/// family (`Opus weekly limit`), in which case that family's window.
fn scope_of(class: LimitClass, lower: &str) -> String {
    if class != LimitClass::Weekly {
        return SCOPE_ALL.to_string();
    }
    patterns()
        .named_weekly
        .captures(lower)
        .and_then(|caps| caps.get(1).or_else(|| caps.get(2)))
        .map(|family| family.as_str().to_string())
        .unwrap_or_else(|| SCOPE_ALL.to_string())
}

/// Read a failure's text: class, scope and — if it says — when it lifts.
///
/// `None` when the text is not a limit. The reset is read against this
/// process' own clock and zone; see [`read_text_at`] for the testable half.
pub fn read_text(text: &str, status: Option<u16>) -> Option<Verdict> {
    read_text_at(text, status, &chrono::Local::now(), local_zone_name())
}

/// [`read_text`] against a given "now" and zone name.
pub fn read_text_at<Tz: TimeZone>(text: &str, status: Option<u16>, now: &DateTime<Tz>, zone: Option<&str>) -> Option<Verdict> {
    let class = classify(text, status)?;
    let lower = text.to_lowercase();
    Some(Verdict {
        class,
        scope: scope_of(class, &lower),
        until_ms: until_in(&lower, now, zone),
    })
}

/// When a limit lifts, by the precedence the contract fixes: the epoch marker,
/// then a `resets <local time>`, then nothing (the caller's default).
fn until_in<Tz: TimeZone>(lower: &str, now: &DateTime<Tz>, zone: Option<&str>) -> Option<i64> {
    let p = patterns();
    if let Some(epoch) = p.epoch.captures(lower).and_then(|caps| caps[1].parse::<i64>().ok()) {
        return Some(epoch * 1000);
    }
    reset_time(lower, now, zone).map(|at| at.timestamp_millis())
}

/// The next time the text's `resets …` names, in `now`'s zone.
///
/// `resets 2:30am (Asia/Tokyo)`, `resets 9pm (Asia/Tokyo)`, `resets Sep 20 at
/// 7am (Asia/Tokyo)`, and a 24-hour `resets 14:30`. A bare hour with no am/pm
/// is refused — `resets 5` could as easily be the start of "5 hours". A zone in
/// parentheses that is not `zone` refuses the whole reading: the time would be
/// right in somebody else's clock and wrong in this one's.
fn reset_time<Tz: TimeZone>(lower: &str, now: &DateTime<Tz>, zone: Option<&str>) -> Option<DateTime<Tz>> {
    let caps = patterns().resets.captures(lower)?;
    if let (Some(named), Some(zone)) = (caps.name("tz"), zone) {
        if !named.as_str().trim().eq_ignore_ascii_case(zone.trim()) {
            return None;
        }
    }
    let hour: u32 = caps.name("h")?.as_str().parse().ok()?;
    let minute: u32 = match caps.name("m") {
        Some(m) => m.as_str().parse().ok()?,
        None => 0,
    };
    let hour24 = match caps.name("ap").map(|ap| ap.as_str()) {
        Some("am") if (1..=12).contains(&hour) => hour % 12,
        Some("pm") if (1..=12).contains(&hour) => hour % 12 + 12,
        Some(_) => return None,
        None if caps.name("m").is_some() && hour < 24 => hour,
        None => return None,
    };
    let time = NaiveTime::from_hms_opt(hour24, minute, 0)?;
    let month_day = match (caps.name("mon"), caps.name("day")) {
        (Some(mon), Some(day)) => Some((month_number(mon.as_str())?, day.as_str().parse::<u32>().ok()?)),
        _ => None,
    };
    next_occurrence(now, month_day, time)
}

/// `jan` → 1 … `dec` → 12.
fn month_number(abbrev: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    MONTHS.iter().position(|m| *m == abbrev).map(|index| index as u32 + 1)
}

/// The wall-clock `time` (on `month_day`, when given) nearest after `now`.
///
/// With no date: today's, or tomorrow's once today's has passed — with a
/// minute's grace, so a message read in the very minute it names is not pushed
/// a whole day out. With a date: whichever year puts it closest to now, which
/// is what makes `resets Jan 2` read on the 30th of December land next week
/// rather than eleven months ago.
fn next_occurrence<Tz: TimeZone>(now: &DateTime<Tz>, month_day: Option<(u32, u32)>, time: NaiveTime) -> Option<DateTime<Tz>> {
    let tz = now.timezone();
    let local = now.naive_local();
    let at = |date: NaiveDate| tz.from_local_datetime(&date.and_time(time)).earliest();
    match month_day {
        None => {
            let grace = now.clone() - chrono::Duration::minutes(1);
            (0..=2u64)
                .filter_map(|days| local.date().checked_add_days(chrono::Days::new(days)))
                .filter_map(at)
                .find(|candidate| *candidate > grace)
        }
        Some((month, day)) => [local.year() - 1, local.year(), local.year() + 1]
            .into_iter()
            .filter_map(|year| NaiveDate::from_ymd_opt(year, month, day))
            .filter_map(at)
            .min_by_key(|candidate| (candidate.timestamp_millis() - now.timestamp_millis()).abs()),
    }
}

/// This process' IANA zone name, when it can be known without a zone database.
///
/// `TZ` first — which is what [`chrono::Local`] reads first too — and otherwise
/// the target of `/etc/localtime`, which on macOS and every mainstream Linux is
/// a symlink into a `zoneinfo/` tree named by the zone. Read once.
pub fn local_zone_name() -> Option<&'static str> {
    static ZONE: OnceLock<Option<String>> = OnceLock::new();
    ZONE.get_or_init(|| {
        let after_zoneinfo = |path: &str| path.rsplit_once("zoneinfo/").map(|(_, name)| name.to_string());
        if let Ok(tz) = std::env::var("TZ") {
            let tz = tz.trim().trim_start_matches(':');
            if !tz.is_empty() {
                return Some(after_zoneinfo(tz).unwrap_or_else(|| tz.to_string()));
            }
        }
        let link = std::fs::read_link("/etc/localtime").ok()?;
        after_zoneinfo(&link.to_string_lossy())
    })
    .as_deref()
}

/// Whether a result the CLI labelled a *success* is nevertheless a limit.
///
/// Deliberately far narrower than [`classify`]: a successful turn's text is the
/// model's own answer, and an answer that *discusses* rate limits is not one.
/// So only the CLI's own sentence counts — short, and at the start — and a 429
/// mentioned in prose never does.
pub fn is_definitive(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.chars().count() <= 300 && patterns().definitive.is_match(&trimmed.to_lowercase())
}

/// The line of `text` that best says why, for a rest's `reason`.
///
/// The first line that is itself a limit of `class`, since a stderr tail puts
/// the exit status first and the 401 somewhere below it; else the first line
/// with anything on it.
pub fn reason_line(text: &str, class: LimitClass) -> String {
    let lines = || text.lines().map(str::trim).filter(|line| !line.is_empty());
    let line = lines()
        .find(|line| classify(line, None) == Some(class))
        .or_else(|| lines().next())
        .unwrap_or_default();
    clip(line, 200)
}

/// The first `cap` characters of `text`, with an ellipsis where it was cut.
pub fn clip(text: &str, cap: usize) -> String {
    if text.chars().count() <= cap {
        return text.to_string();
    }
    text.chars().take(cap.saturating_sub(1)).chain(['…']).collect()
}

/// An epoch in seconds or milliseconds, as milliseconds.
fn epoch_ms(value: &Value) -> Option<i64> {
    let raw = value.as_i64().or_else(|| value.as_f64().map(|f| f as i64))?;
    if raw <= 0 {
        None
    } else if raw > 1_000_000_000_000 {
        Some(raw)
    } else {
        Some(raw * 1000)
    }
}

// ------------------------------------------------------------ payloads

/// A claude `rate_limit_event`'s `rate_limit_info`, when it is a refusal.
///
/// claude writes one of these before nearly every turn, and almost all of them
/// say `status: allowed` — or `allowed_warning` as a window fills up. Neither
/// is a limit, and recording them would bury the one that is. A refusal names
/// its window and when it resets:
///
/// ```text
/// {"status":"rejected","resetsAt":1789534200,"rateLimitType":"five_hour",…}
/// ```
///
/// `five_hour` is the session window and `seven_day*` a weekly one: `seven_day`
/// itself is every model's and scoped to the whole account, and a
/// `seven_day_<family>` window is scoped to that family. A window type this
/// module has never seen is a plain rate limit.
///
/// Any *other* `seven_day_*` window — `seven_day_overage_included` is the one
/// seen so far — covers models its name does not give, so it comes back with a
/// [`WindowHint`] and scope [`SCOPE_UNKNOWN`]: log it, rest nothing, and let the
/// `result` that follows decide (see the module note).
pub fn read_rate_limit_event(info: &Value) -> Option<RateLimitReading> {
    let status = info.get("status").and_then(Value::as_str)?;
    if status.starts_with("allowed") {
        return None;
    }
    let kind = info.get("rateLimitType").and_then(Value::as_str).unwrap_or_default();
    let until_ms = info.get("resetsAt").and_then(epoch_ms);
    let reading = |class: LimitClass, scope: &str, hint: Option<WindowHint>| RateLimitReading {
        verdict: Verdict {
            class,
            scope: scope.to_string(),
            until_ms,
        },
        hint,
    };
    Some(if kind == "five_hour" {
        reading(LimitClass::Session, SCOPE_ALL, None)
    } else if kind == "seven_day" {
        reading(LimitClass::Weekly, SCOPE_ALL, None)
    } else if let Some(suffix) = kind.strip_prefix("seven_day_") {
        match ["fable", "opus", "sonnet", "haiku"].into_iter().find(|family| *family == suffix) {
            Some(family) => reading(LimitClass::Weekly, family, None),
            None => reading(
                LimitClass::Weekly,
                SCOPE_UNKNOWN,
                Some(WindowHint {
                    window: kind.to_string(),
                    resets_at_ms: until_ms,
                }),
            ),
        }
    } else {
        reading(LimitClass::Rate, SCOPE_ALL, None)
    })
}

/// What a refused `rate_limit_event` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitReading {
    /// What to log. Scope [`SCOPE_UNKNOWN`] exactly when `hint` is set.
    pub verdict: Verdict,
    /// Set for a weekly window whose name does not say which models it
    /// covers: the event must not rest the account, and this is what the
    /// session's next `result` borrows its reset time from.
    pub hint: Option<WindowHint>,
}

/// A refused weekly window of unknown scope, held for the `result` after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowHint {
    /// The window's own name, e.g. `seven_day_overage_included`.
    pub window: String,
    /// When it resets, epoch milliseconds, if the event said.
    pub resets_at_ms: Option<i64>,
}

/// Where a session's driver keeps a [`WindowHint`] until its `result` arrives.
///
/// In memory and nowhere else: it lives for ten seconds, and a restart in
/// between loses a reset time, not a fact — the event itself is in the log.
#[derive(Debug, Default)]
pub struct HintSlot(Option<(Instant, WindowHint)>);

impl HintSlot {
    /// Hold `hint`, replacing whatever was held.
    pub fn hold(&mut self, hint: WindowHint, at: Instant) {
        self.0 = Some((at, hint));
    }

    /// The held hint if it is at most [`HINT_WINDOW`] old at `at`; the slot is
    /// emptied either way, because a hint explains one result and no other.
    pub fn take(&mut self, at: Instant) -> Option<WindowHint> {
        let (held, hint) = self.0.take()?;
        (at.saturating_duration_since(held) <= HINT_WINDOW).then_some(hint)
    }
}

/// A `result`'s verdict, with the reset time a [`WindowHint`] held for it.
///
/// Only a weekly result takes it — the hint is a weekly window, and lending
/// three days to a rate limit or an auth failure would be inventing a fact.
/// The result's own scope stands (a family it names, or `all`), and so does a
/// reset time in its own words; the hint only fills a gap the words left.
pub fn with_hint(verdict: Verdict, hint: Option<&WindowHint>) -> Verdict {
    match hint {
        Some(hint) if verdict.class == LimitClass::Weekly && verdict.until_ms.is_none() => Verdict {
            until_ms: hint.resets_at_ms,
            ..verdict
        },
        _ => verdict,
    }
}

/// A codex `turn/completed` with `status: failed`, from its `turn.error`.
///
/// `codexErrorInfo` is read before the words: it is codex's own name for what
/// went wrong, and it names the context-window case outright so the exclusion
/// cannot be missed. The words still decide the rest — a usage limit that says
/// "weekly", a reset time — and anything the code does not settle.
pub fn read_codex_turn_error(error: &Value) -> Option<Verdict> {
    read_codex_turn_error_at(error, &chrono::Local::now(), local_zone_name())
}

/// [`read_codex_turn_error`] against a given "now" and zone name.
pub fn read_codex_turn_error_at<Tz: TimeZone>(error: &Value, now: &DateTime<Tz>, zone: Option<&str>) -> Option<Verdict> {
    let text = codex_error_text(error);
    let (code, status) = codex_error_code(error.get("codexErrorInfo"));
    let structured = match code.as_deref() {
        Some("contextWindowExceeded") | Some("sessionBudgetExceeded") => return None,
        Some("usageLimitExceeded") => Some(LimitClass::Session),
        Some("rateLimitExceeded") | Some("serverOverloaded") => Some(LimitClass::Rate),
        Some("unauthorized") => Some(LimitClass::Auth),
        _ => None,
    };
    let textual = read_text_at(&text, status, now, zone);
    let Some(class) = structured else { return textual };
    let lower = text.to_lowercase();
    // A usage limit the words call weekly is a weekly one; otherwise the code
    // is the class and the words only contribute the reset.
    let class = match (class, textual.as_ref().map(|v| v.class)) {
        (LimitClass::Session, Some(LimitClass::Weekly)) => LimitClass::Weekly,
        _ => class,
    };
    Some(Verdict {
        class,
        scope: scope_of(class, &lower),
        until_ms: until_in(&lower, now, zone),
    })
}

/// The words of a codex turn error: its message and any detail beside it.
fn codex_error_text(error: &Value) -> String {
    if let Some(text) = error.as_str() {
        return text.to_string();
    }
    ["message", "additionalDetails"]
        .iter()
        .filter_map(|key| error.get(*key).and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `codexErrorInfo` as a code and, for its HTTP variants, the upstream status.
fn codex_error_code(info: Option<&Value>) -> (Option<String>, Option<u16>) {
    match info {
        Some(Value::String(code)) => (Some(code.clone()), None),
        Some(Value::Object(map)) => {
            let status = map
                .values()
                .find_map(|inner| inner.get("httpStatusCode").and_then(Value::as_u64))
                .and_then(|status| u16::try_from(status).ok());
            (map.keys().next().cloned(), status)
        }
        _ => (None, None),
    }
}

/// A codex `account/rateLimits/updated`, when it says a limit has been reached.
///
/// codex sends one of these after every model response — a sparse rolling
/// update of the account's windows — so an update that is merely a reading is
/// not an event. One is worth recording when a window stands at 100 %, or
/// codex says outright that a limit or a spend control was reached. The rest
/// lasts until the earliest reset among the full windows; with none given the
/// event is still recorded, and the caller leaves the account unrested.
pub fn read_codex_rate_limits(params: &Value) -> Option<Verdict> {
    let limits = params.get("rateLimits")?;
    let full: Vec<&Value> = ["primary", "secondary"]
        .iter()
        .filter_map(|key| limits.get(*key))
        .filter(|window| window.get("usedPercent").and_then(Value::as_f64).is_some_and(|used| used >= 100.0))
        .collect();
    let reached = limits.get("rateLimitReachedType").and_then(Value::as_str);
    let spend = limits.get("spendControlReached").and_then(Value::as_bool) == Some(true);
    if full.is_empty() && reached.is_none() && !spend {
        return None;
    }
    let class = if spend || reached.is_some_and(|kind| kind.contains("credits")) {
        LimitClass::Credit
    } else {
        LimitClass::Rate
    };
    Some(Verdict {
        class,
        scope: SCOPE_ALL.to_string(),
        until_ms: full.iter().filter_map(|window| window.get("resetsAt").and_then(epoch_ms)).min(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;
    use serde_json::json;

    /// Tokyo has no daylight saving, so a fixed +09:00 *is* Asia/Tokyo — which
    /// is what lets these tests run the same on any machine in any zone.
    fn tokyo(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(9 * 3600)
            .unwrap()
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .unwrap()
    }

    const ZONE: Option<&str> = Some("Asia/Tokyo");

    fn read(text: &str, now: &DateTime<FixedOffset>) -> Option<Verdict> {
        read_text_at(text, None, now, ZONE)
    }

    #[test]
    fn the_session_limit_wording_is_a_session_limit_with_its_reset_read_in_tokyo() {
        let now = tokyo(2026, 9, 25, 23, 0);
        let verdict = read("You've hit your session limit · resets 2:30am (Asia/Tokyo)", &now).expect("a limit");
        assert_eq!(verdict.class, LimitClass::Session);
        assert_eq!(verdict.scope, "all", "the five-hour window covers every model");
        assert_eq!(
            verdict.until_ms,
            Some(tokyo(2026, 9, 26, 2, 30).timestamp_millis()),
            "2:30am after 11pm is tomorrow's"
        );

        // The same wording earlier in the day is today's, and it matches the
        // epoch the matching `rate_limit_event` carried (1789534200).
        let morning = tokyo(2026, 9, 16, 12, 0);
        let verdict = read("You've hit your session limit · resets 1:50pm (Asia/Tokyo)", &morning).unwrap();
        assert_eq!(verdict.until_ms, Some(1_789_534_200_000));

        for (text, hour, minute) in [
            ("You've hit your session limit · resets 9pm (Asia/Tokyo)", 21, 0),
            ("You've hit your session limit · resets 12am (Asia/Tokyo)", 0, 0),
            ("You've hit your session limit · resets 12:40am (Asia/Tokyo)", 0, 40),
            ("You've hit your session limit · resets 12pm (Asia/Tokyo)", 12, 0),
        ] {
            let at = read(text, &morning).unwrap().until_ms.expect(text);
            let expected = next_occurrence(&morning, None, NaiveTime::from_hms_opt(hour, minute, 0).unwrap()).unwrap();
            assert_eq!(at, expected.timestamp_millis(), "{text}");
        }
    }

    #[test]
    fn the_weekly_wording_is_weekly_for_the_whole_account_and_reads_its_date() {
        let now = tokyo(2026, 9, 16, 12, 0);
        let verdict = read("You've hit your weekly limit · resets Sep 20 at 7am (Asia/Tokyo)", &now).unwrap();
        assert_eq!(verdict.class, LimitClass::Weekly);
        assert_eq!(verdict.scope, "all", "no family named: the seven-day window every model shares");
        // 1789855200 is what the matching `rate_limit_event` said.
        assert_eq!(verdict.until_ms, Some(1_789_855_200_000));

        // A family in the words wins over the model's.
        let opus = read("You've hit your Opus weekly limit · resets Oct 3 at 2pm (Asia/Tokyo)", &now).unwrap();
        assert_eq!((opus.class, opus.scope.as_str()), (LimitClass::Weekly, "opus"));
        let sonnet = read("You've hit your Sonnet limit", &now).unwrap();
        assert_eq!((sonnet.class, sonnet.scope.as_str()), (LimitClass::Weekly, "sonnet"));

        let fable = read("You've hit your weekly Fable limit", &now).unwrap();
        assert_eq!(fable.scope, "fable");
        let haiku = read("Haiku weekly limit reached", &now).unwrap();
        assert_eq!(haiku.scope, "haiku");
        // A family mentioned somewhere else in the text does not scope it.
        let elsewhere = read("You've hit your weekly limit. Try Sonnet instead.", &now).unwrap();
        assert_eq!((elsewhere.class, elsewhere.scope.as_str()), (LimitClass::Weekly, "all"));

        // A date that has just passed is this year's, not next year's; one
        // across the new year is next year's, not last January's.
        let late = tokyo(2026, 12, 30, 10, 0);
        let wrapped = read("You've hit your weekly limit · resets Jan 2 at 9am (Asia/Tokyo)", &late).unwrap();
        assert_eq!(wrapped.until_ms, Some(tokyo(2027, 1, 2, 9, 0).timestamp_millis()));
    }

    #[test]
    fn a_reset_in_another_zone_is_left_unparsed() {
        let now = tokyo(2026, 9, 16, 12, 0);
        let verdict = read("You've hit your session limit · resets 2:30am (America/New_York)", &now).unwrap();
        assert_eq!(verdict.class, LimitClass::Session, "still a limit");
        assert_eq!(verdict.until_ms, None, "but its time is somebody else's clock");

        // No zone in the text, or no zone known here: the CLI printed this
        // machine's time, so it is read as such.
        assert!(read("session limit · resets 2:30am", &now).unwrap().until_ms.is_some());
        let unknown_zone = read_text_at("session limit · resets 2:30am (Asia/Tokyo)", None, &now, None).unwrap();
        assert!(unknown_zone.until_ms.is_some());

        // A bare hour is not a time.
        assert_eq!(read("usage limit, resets 5 hours from now", &now).unwrap().until_ms, None);
        // A 24-hour time with minutes is.
        assert_eq!(
            read("usage limit · resets 14:30", &now).unwrap().until_ms,
            Some(tokyo(2026, 9, 16, 14, 30).timestamp_millis())
        );
    }

    #[test]
    fn the_epoch_marker_wins_over_a_reset_time() {
        let now = tokyo(2026, 9, 16, 12, 0);
        let verdict = read("Claude AI usage limit reached|1790157600", &now).unwrap();
        assert_eq!(verdict.class, LimitClass::Session);
        assert_eq!(verdict.until_ms, Some(1_790_157_600_000));

        let both = read("Claude AI usage limit reached|1790157600 · resets 2:30am (Asia/Tokyo)", &now).unwrap();
        assert_eq!(both.until_ms, Some(1_790_157_600_000), "the epoch is exact; the wall clock is rounded");
        assert!(is_definitive("Claude AI usage limit reached|1790157600"));
    }

    #[test]
    fn auth_wordings_are_auth() {
        for text in [
            "401 OAuth access token is invalid.",
            "API Error: 401 Invalid bearer token",
            "OAuth session expired and could not be refreshed",
            "Not logged in · Please run /login",
            "Invalid API key · Please run /login",
            r#"API Error: 401 {"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#,
        ] {
            let verdict = read(text, &tokyo(2026, 9, 16, 12, 0)).expect(text);
            assert_eq!((verdict.class, verdict.scope.as_str()), (LimitClass::Auth, "all"), "{text}");
        }
        // A longer number with 401 inside it is not a status.
        assert_eq!(classify("request 14012 finished", None), None);
    }

    #[test]
    fn credit_and_rate_wordings() {
        for text in [
            "Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing.",
            "insufficient_quota: You exceeded your current quota, please check your plan and billing details.",
            "账户余额不足",
            "账号已欠费",
        ] {
            assert_eq!(classify(text, None), Some(LimitClass::Credit), "{text}");
        }
        for text in [
            "API Error: 429 Too Many Requests",
            "This request would exceed the rate limit for your organization",
            r#"{"type":"error","error":{"type":"rate_limit_error"}}"#,
            "API Error: 529 Overloaded. This is a server-side issue, usually temporary",
        ] {
            assert_eq!(classify(text, None), Some(LimitClass::Rate), "{text}");
        }
    }

    #[test]
    fn what_is_not_a_limit_is_not_one() {
        // Real claude 2.1.266 failures that are nobody's quota, with the status
        // each one actually carried.
        for (text, status) in [
            (
                "There's an issue with the selected model (claude-fable-5-1). It may not exist or you may not have access to it. Run /model to pick a different model.",
                Some(404),
            ),
            ("[claude-code:unrecognized_model] claude-fable-9", None),
            ("API Error: Server error mid-response. The response above may be incomplete.", None),
            ("", None),
        ] {
            assert_eq!(classify(text, status), None, "{text}");
        }
        // Context-length failures say "limit", and must still not count — even
        // when a 429 or "rate limit" rides along.
        for text in [
            "Prompt is too long · automatic compaction failed: API Error: 429 rate limit",
            "input length and max_tokens exceed context limit: 190000 + 32000 > 200000",
            "This model's maximum context window is exceeded",
            "token limit exceeded for this request",
        ] {
            assert_eq!(classify(text, None), None, "{text}");
            assert_eq!(classify(text, Some(429)), None, "a status does not override an exclusion: {text}");
        }
    }

    #[test]
    fn a_status_breaks_a_tie_the_words_could_not() {
        assert_eq!(classify("something new happened", Some(429)), Some(LimitClass::Rate));
        assert_eq!(classify("something new happened", Some(401)), Some(LimitClass::Auth));
        assert_eq!(classify("something new happened", Some(402)), Some(LimitClass::Credit));
        assert_eq!(classify("something new happened", Some(500)), None);
        // Limit-shaped wording nothing more specific matches is `other`.
        assert_eq!(classify("You've reached your limit for today", None), Some(LimitClass::Other));
        assert_eq!(classify("quota exhausted", None), Some(LimitClass::Other));
        // …unless it says when it resets, which makes it a usage window.
        assert_eq!(classify("Limit reached · resets 5pm", None), Some(LimitClass::Session));
    }

    #[test]
    fn the_defaults_and_the_other_backoff() {
        assert_eq!(default_rest(LimitClass::Session, 0), Rest::For(Duration::from_secs(30 * 60)));
        assert_eq!(default_rest(LimitClass::Weekly, 0), Rest::For(Duration::from_secs(60 * 60)));
        assert_eq!(default_rest(LimitClass::Rate, 0), Rest::For(Duration::from_secs(60)));
        assert_eq!(default_rest(LimitClass::Credit, 0), Rest::For(Duration::from_secs(30 * 60)));
        assert_eq!(default_rest(LimitClass::Auth, 0), Rest::Indefinite);

        assert_eq!(default_rest(LimitClass::Other, 0), Rest::Skip, "a one-off is recorded, not rested");
        let minutes = |streak| match default_rest(LimitClass::Other, streak) {
            Rest::For(duration) => duration.as_secs() / 60,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            (1..=7).map(minutes).collect::<Vec<_>>(),
            vec![1, 2, 4, 8, 10, 10, 10],
            "doubling from one minute, capped at ten"
        );
        assert_eq!(minutes(u32::MAX), 10, "a very long streak neither overflows nor escapes the cap");
    }

    #[test]
    fn only_the_clis_own_sentence_makes_a_success_a_limit() {
        assert!(is_definitive("You've hit your session limit · resets 2:30am (Asia/Tokyo)"));
        assert!(is_definitive("Error: You've hit your session limit · resets 2:30am (Asia/Tokyo)"));
        assert!(is_definitive("You’ve hit your weekly limit · resets Sep 20 at 7am (Asia/Tokyo)"));
        assert!(is_definitive("You've reached your Fable limit. Switch to another model, or manage usage credits at claude.ai/settings/usage?from=cc_cli_limit_message, to continue."));
        // A reply that talks about limits is a reply.
        assert!(!is_definitive("The API returned 429 Too Many Requests, so I added a retry."));
        assert!(!is_definitive("If you've hit your session limit, the CLI says so and the turn fails."));
        assert!(!is_definitive(&format!("You've hit your session limit {}", "x".repeat(400))));
    }

    #[test]
    fn a_rejected_rate_limit_event_names_its_window() {
        let session = json!({
            "status": "rejected", "resetsAt": 1789534200, "rateLimitType": "five_hour",
            "overageStatus": "rejected", "isUsingOverage": false,
            "unifiedWindows": {"five_hour": {"utilization": 1, "resetsAt": 1789534200}},
        });
        let reading = read_rate_limit_event(&session).unwrap();
        assert_eq!(
            reading.verdict,
            Verdict {
                class: LimitClass::Session,
                scope: "all".into(),
                until_ms: Some(1_789_534_200_000)
            }
        );
        assert_eq!(reading.hint, None);

        let weekly = json!({"status": "rejected", "resetsAt": 1789855200, "rateLimitType": "seven_day"});
        let verdict = read_rate_limit_event(&weekly).unwrap().verdict;
        assert_eq!(
            (verdict.class, verdict.scope.as_str()),
            (LimitClass::Weekly, "all"),
            "`seven_day` is the window every model shares"
        );
        let opus = json!({"status": "rejected", "rateLimitType": "seven_day_opus"});
        let reading = read_rate_limit_event(&opus).unwrap();
        assert_eq!(
            (reading.verdict.class, reading.verdict.scope.as_str(), reading.verdict.until_ms),
            (LimitClass::Weekly, "opus", None)
        );
        assert_eq!(reading.hint, None, "a family in the window's name is a scope");
        let sonnet = json!({"status": "rejected", "rateLimitType": "seven_day_sonnet"});
        assert_eq!(read_rate_limit_event(&sonnet).unwrap().verdict.scope, "sonnet");
        let novel = json!({"status": "rejected", "rateLimitType": "per_minute"});
        assert_eq!(read_rate_limit_event(&novel).unwrap().verdict.class, LimitClass::Rate);

        // The line claude writes before nearly every turn is not a limit.
        for status in ["allowed", "allowed_warning"] {
            let info = json!({"status": status, "resetsAt": 1790157600, "rateLimitType": "five_hour"});
            assert_eq!(read_rate_limit_event(&info), None, "{status}");
        }
        assert_eq!(read_rate_limit_event(&json!({})), None, "no status, no verdict");
    }

    /// The first real Fable weekly hit, verbatim: the event names a window that
    /// does not say whose it is, and the result names the family but no time.
    #[test]
    fn a_weekly_window_of_unknown_scope_is_a_hint_and_the_result_supplies_the_family() {
        let event = json!({
            "isUsingOverage": false, "overageDisabledReason": "org_level_disabled", "overageStatus": "rejected",
            "rateLimitType": "seven_day_overage_included", "resetsAt": 1790575200, "status": "rejected",
            "unifiedWindows": {
                "five_hour": {"resetsAt": 1790436000, "utilization": 0.02},
                "seven_day": {"resetsAt": 1790575200, "utilization": 0.82},
                "seven_day_overage_included": {"resetsAt": 1790575200, "utilization": 1},
            },
        });
        let reading = read_rate_limit_event(&event).unwrap();
        assert_eq!(
            (reading.verdict.class, reading.verdict.scope.as_str()),
            (LimitClass::Weekly, SCOPE_UNKNOWN),
            "the general seven-day window was at 82 % and still usable: this is not `all`"
        );
        let hint = reading.hint.expect("a window of unknown scope is a hint");
        assert_eq!(
            hint,
            WindowHint {
                window: "seven_day_overage_included".into(),
                resets_at_ms: Some(1_790_575_200_000)
            }
        );

        let now = tokyo(2026, 9, 25, 12, 0);
        let text = "You've reached your Fable limit. Switch to another model, or manage usage credits at claude.ai/settings/usage?from=cc_cli_limit_message, to continue.";
        let result = read_text_at(text, Some(429), &now, ZONE).expect("a limit");
        assert_eq!(
            result,
            Verdict {
                class: LimitClass::Weekly,
                scope: "fable".into(),
                until_ms: None
            },
            "\"credits\" in the sentence does not make it a credit limit"
        );
        assert!(is_definitive(text), "the CLI's own sentence, even labelled a success");

        // Merged: the result's family, the event's time.
        assert_eq!(
            with_hint(result.clone(), Some(&hint)),
            Verdict {
                class: LimitClass::Weekly,
                scope: "fable".into(),
                until_ms: Some(1_790_575_200_000)
            }
        );
        // No hint: the class default applies downstream.
        assert_eq!(with_hint(result, None).until_ms, None);

        // A result that names no family takes the whole account, with the time.
        let plain = read_text_at("You've hit your weekly limit", None, &now, ZONE).unwrap();
        let merged = with_hint(plain, Some(&hint));
        assert_eq!((merged.scope.as_str(), merged.until_ms), ("all", Some(1_790_575_200_000)));
        // The result's own reset time beats the hint's.
        let timed = read_text_at("You've reached your Fable limit · resets Sep 26 at 9am (Asia/Tokyo)", None, &now, ZONE).unwrap();
        assert_eq!(with_hint(timed, Some(&hint)).until_ms, Some(tokyo(2026, 9, 26, 9, 0).timestamp_millis()));
        // A result of another class borrows nothing.
        let session = read_text_at("You've hit your session limit", None, &now, ZONE).unwrap();
        assert_eq!(with_hint(session, Some(&hint)).until_ms, None);
    }

    #[test]
    fn a_hint_explains_one_result_within_ten_seconds() {
        let hint = WindowHint {
            window: "seven_day_overage_included".into(),
            resets_at_ms: Some(1_790_575_200_000),
        };
        let start = Instant::now();
        let mut slot = HintSlot::default();
        assert_eq!(slot.take(start), None);

        slot.hold(hint.clone(), start);
        assert_eq!(slot.take(start + Duration::from_secs(10)), Some(hint.clone()), "ten seconds is inside");
        assert_eq!(slot.take(start + Duration::from_secs(10)), None, "and it is used up");

        slot.hold(hint, start);
        assert_eq!(slot.take(start + Duration::from_millis(10_001)), None, "a stale hint is dropped");
        assert_eq!(slot.take(start), None, "and does not come back");
    }

    #[test]
    fn a_codex_turn_error_is_read_by_its_code_first() {
        let now = tokyo(2026, 9, 16, 12, 0);
        let read = |error: Value| read_codex_turn_error_at(&error, &now, ZONE);

        let usage = read(json!({
            "message": "You've hit your usage limit. Upgrade to Pro or try again in 4 days.",
            "codexErrorInfo": "usageLimitExceeded",
        }))
        .unwrap();
        assert_eq!((usage.class, usage.scope.as_str()), (LimitClass::Session, "all"));
        let rate = read(json!({"message": "slow down", "codexErrorInfo": "rateLimitExceeded"})).unwrap();
        assert_eq!(rate.class, LimitClass::Rate);
        let auth = read(json!({"message": "nope", "codexErrorInfo": "unauthorized"})).unwrap();
        assert_eq!(auth.class, LimitClass::Auth);
        assert_eq!(
            read(json!({"message": "rate limit", "codexErrorInfo": "contextWindowExceeded"})),
            None,
            "the context window is excluded by name, whatever the words say"
        );
        let http = read(json!({
            "message": "stream failed",
            "codexErrorInfo": {"responseStreamConnectionFailed": {"httpStatusCode": 429}},
        }))
        .unwrap();
        assert_eq!(http.class, LimitClass::Rate, "the upstream status breaks the tie");
        assert_eq!(read(json!({"message": "stream disconnected", "codexErrorInfo": "other"})), None);
        assert_eq!(read(json!("401 Unauthorized")).unwrap().class, LimitClass::Auth, "a bare string error");
    }

    #[test]
    fn a_codex_rate_limit_update_counts_only_at_a_full_window() {
        let reading = json!({"rateLimits": {
            "primary": {"usedPercent": 40, "windowDurationMins": 300, "resetsAt": 1790000000},
            "secondary": {"usedPercent": 83, "windowDurationMins": 10080, "resetsAt": 1790500000},
        }});
        assert_eq!(read_codex_rate_limits(&reading), None, "a reading is not an event");

        let full = json!({"rateLimits": {
            "primary": {"usedPercent": 100, "windowDurationMins": 300, "resetsAt": 1790000000},
            "secondary": {"usedPercent": 100, "windowDurationMins": 10080, "resetsAt": 1790500000},
        }});
        assert_eq!(
            read_codex_rate_limits(&full),
            Some(Verdict {
                class: LimitClass::Rate,
                scope: "all".into(),
                until_ms: Some(1_790_000_000_000)
            }),
            "the earliest reset among the full windows"
        );

        let no_reset = json!({"rateLimits": {"primary": {"usedPercent": 100}}});
        assert_eq!(read_codex_rate_limits(&no_reset).unwrap().until_ms, None, "an event without a rest");

        let credits = json!({"rateLimits": {"rateLimitReachedType": "workspace_member_credits_depleted"}});
        assert_eq!(read_codex_rate_limits(&credits).unwrap().class, LimitClass::Credit);
        assert_eq!(read_codex_rate_limits(&json!({})), None);
    }

    #[test]
    fn a_reason_is_the_line_that_says_why() {
        let tail = "claude exited with code 1\nstarting up\n401 OAuth access token is invalid.";
        assert_eq!(reason_line(tail, LimitClass::Auth), "401 OAuth access token is invalid.");
        assert_eq!(reason_line("\n  first\nsecond", LimitClass::Rate), "first");
        assert_eq!(clip("abcdef", 4), "abc…");
        assert_eq!(clip("abc", 4), "abc");
    }
}
