//! The cron expression, and the one question the scheduler asks it.
//!
//! Five fields — minute, hour, day-of-month, month, day-of-week — parsed by
//! [`croner`]. Seconds are *disallowed* rather than merely unused: croner accepts
//! a leading seconds field when one is present, and a pattern like `*/10 * * * * *`
//! would fire six times a minute against a runner that ticks every five seconds.
//! A scheduled task starts a CLI, which costs tokens and a process, so the
//! smallest grain this feature offers is a minute and the parser refuses to
//! pretend otherwise. A trailing year field is refused for the same reason: it
//! parses, but it means something the UI cannot show.
//!
//! # Time zone: the server's own
//!
//! `0 9 * * *` means nine in the morning **where the server is**, which is what
//! `crontab` has always meant and what a user writing a schedule expects. A
//! scheduler that evaluates every expression in UTC fires a "9am" task at 5pm
//! for anyone east of Greenwich. The cost
//! is that the meaning of a stored expression follows the machine's `TZ`; the
//! benefit is that nobody has to do arithmetic to write one down.
//!
//! No `chrono-tz`, and no per-task zone. One more field on the form, one more
//! thing to get wrong, and a database of zone rules in the binary, all to serve
//! the case where the user is in a different zone from their own server — which
//! for a tool that drives *that server's* CLIs is not the common one.
//!
//! # Daylight saving
//!
//! Handled by croner rather than by us, and not specially: it resolves each
//! candidate wall-clock time through `chrono`'s `from_local_datetime`, so an hour
//! that a spring-forward skipped simply has no occurrence that day and an hour a
//! fall-back repeated is matched once. A daily task therefore runs 23 or 25 hours
//! after its predecessor across a shift, and a task scheduled inside a skipped
//! hour does not run on the day it is skipped. Both are the behaviour `cron`
//! itself has, and neither is worth a special case: the alternative is deciding
//! on the user's behalf whether "9am" meant the wall clock or the elapsed hours,
//! and the wall clock is what they wrote down.

use std::sync::LazyLock;
use std::time::SystemTime;

use chrono::{DateTime, Local, TimeZone};
use croner::parser::{CronParser, Seconds, Year};
use croner::Cron;

/// The longest expression accepted, in characters.
///
/// A five-field pattern with every list spelled out is well under this; the cap
/// exists because the string is stored, re-parsed on every tick and echoed to the
/// UI, and croner's parser is happy to work through something pathological.
pub const MAX_EXPR: usize = 200;

/// The parser every expression in this crate goes through.
///
/// Built once: it is five booleans, but `CronParserBuilder` is not `const`.
static PARSER: LazyLock<CronParser> = LazyLock::new(|| CronParser::builder().seconds(Seconds::Disallowed).year(Year::Disallowed).build());

/// Parse a five-field expression, or say what is wrong with it.
///
/// The error is croner's own text, which names the offending field, and it goes
/// straight to the user: they are looking at the expression they just typed.
pub fn parse(expr: &str) -> Result<Cron, String> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Err("the schedule is empty".to_string());
    }
    if trimmed.chars().count() > MAX_EXPR {
        return Err("the schedule is too long".to_string());
    }
    PARSER.parse(trimmed).map_err(|error| error.to_string())
}

/// The first time `expr` fires strictly after `now`, in `now`'s own time zone.
///
/// Generic over the zone so the tests can pin one: with `Local` the answer would
/// depend on the machine running them, and a scheduler whose month-end and
/// year-rollover behaviour is only asserted in one zone is a scheduler with
/// untested arithmetic.
///
/// `None` covers both refusals — an expression that does not parse, and one that
/// parses but has no future occurrence (`0 0 30 2 *`) — because the caller does
/// the same thing with either: it does not schedule.
pub fn next_after<Tz: TimeZone>(expr: &str, now: &DateTime<Tz>) -> Option<DateTime<Tz>> {
    parse(expr).ok()?.find_next_occurrence(now, false).ok()
}

/// The same question against the server's local clock, in the units the store
/// and the runner speak.
pub fn next_occurrence(expr: &str, now: SystemTime) -> Option<SystemTime> {
    let now: DateTime<Local> = now.into();
    next_after(expr, &now).map(SystemTime::from)
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;

    /// A wall clock reading in a zone that has no daylight saving, so every
    /// assertion below is about the calendar arithmetic and nothing else.
    fn at(y: i32, m: u32, d: u32, hh: u32, mm: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(8 * 3600)
            .expect("a valid offset")
            .with_ymd_and_hms(y, m, d, hh, mm, 0)
            .single()
            .expect("an unambiguous instant")
    }

    #[test]
    fn a_five_field_expression_parses_and_anything_else_does_not() {
        assert!(parse("0 9 * * *").is_ok());
        assert!(parse("  */15 * * * *  ").is_ok(), "surrounding space is not a syntax error");
        assert!(parse("0 9 * * MON").is_ok(), "weekday names are croner's own alias table");
        assert!(parse("@daily").is_ok(), "a nickname expands to five fields");

        for bad in [
            "",
            "   ",
            "not a cron",
            "* * * *",
            "0 9 * * * *",
            "0 9 * * * * *",
            "60 * * * *",
            "0 25 * * *",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn a_six_field_expression_is_refused_rather_than_read_as_seconds() {
        // The whole reason `Seconds::Disallowed` is set: croner would otherwise
        // read this as "every ten seconds", and every occurrence of it starts a
        // CLI.
        assert!(parse("*/10 * * * * *").is_err());
    }

    #[test]
    fn a_daily_expression_lands_on_the_next_day_when_today_is_spent() {
        let next = next_after("0 9 * * *", &at(2026, 3, 1, 10, 0)).expect("an occurrence");
        assert_eq!(next, at(2026, 3, 2, 9, 0));

        let next = next_after("0 9 * * *", &at(2026, 3, 1, 8, 30)).expect("an occurrence");
        assert_eq!(next, at(2026, 3, 1, 9, 0), "later today still counts");
    }

    #[test]
    fn the_search_is_strictly_after_now() {
        // Standing exactly on an occurrence must yield the *next* one, or a
        // runner that advances before it fires would fire the same minute twice.
        let next = next_after("0 9 * * *", &at(2026, 3, 1, 9, 0)).expect("an occurrence");
        assert_eq!(next, at(2026, 3, 2, 9, 0));
    }

    #[test]
    fn a_day_of_month_that_some_months_lack_skips_them() {
        let next = next_after("0 9 31 * *", &at(2026, 1, 31, 10, 0)).expect("an occurrence");
        assert_eq!(next, at(2026, 3, 31, 9, 0), "february and the 31st never meet");

        // The last day of a month, whichever day that is.
        let next = next_after("0 9 L * *", &at(2026, 2, 1, 0, 0)).expect("an occurrence");
        assert_eq!(next, at(2026, 2, 28, 9, 0));
    }

    #[test]
    fn a_leap_day_waits_for_a_leap_year() {
        let next = next_after("0 0 29 2 *", &at(2026, 3, 1, 0, 0)).expect("an occurrence");
        assert_eq!(next, at(2028, 2, 29, 0, 0));
    }

    #[test]
    fn a_yearly_expression_rolls_over() {
        let next = next_after("0 0 1 1 *", &at(2026, 6, 15, 12, 0)).expect("an occurrence");
        assert_eq!(next, at(2027, 1, 1, 0, 0));
    }

    #[test]
    fn a_step_lands_on_the_next_multiple_rather_than_now_plus_the_step() {
        let next = next_after("*/15 * * * *", &at(2026, 3, 1, 10, 7)).expect("an occurrence");
        assert_eq!(next, at(2026, 3, 1, 10, 15));

        let next = next_after("0 */3 * * *", &at(2026, 3, 1, 10, 7)).expect("an occurrence");
        assert_eq!(next, at(2026, 3, 1, 12, 0));
    }

    #[test]
    fn a_weekday_expression_finds_the_named_day() {
        // 2026-03-01 is a Sunday, so the next Monday is the 2nd.
        let next = next_after("0 9 * * 1", &at(2026, 3, 1, 12, 0)).expect("an occurrence");
        assert_eq!(next, at(2026, 3, 2, 9, 0));
        assert_eq!(next_after("0 9 * * MON", &at(2026, 3, 1, 12, 0)), Some(at(2026, 3, 2, 9, 0)));
    }

    #[test]
    fn an_unparseable_expression_has_no_next_occurrence() {
        assert_eq!(next_after("nonsense", &at(2026, 3, 1, 12, 0)), None);
        assert_eq!(next_occurrence("nonsense", SystemTime::now()), None);
    }

    #[test]
    fn an_expression_that_can_never_match_has_no_next_occurrence() {
        // February has no 30th, in any year.
        assert_eq!(next_after("0 0 30 2 *", &at(2026, 3, 1, 12, 0)), None);
    }

    #[test]
    fn the_local_clock_answer_is_in_the_future() {
        let now = SystemTime::now();
        let next = next_occurrence("* * * * *", now).expect("an occurrence");
        assert!(next > now, "a minute-grained schedule always has a next minute");
        assert!(
            next.duration_since(now).expect("in the future").as_secs() <= 60,
            "and it is the *next* one"
        );
    }
}
