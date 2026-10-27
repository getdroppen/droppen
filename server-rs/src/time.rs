//! Wall-clock readings, and the ISO-8601 spelling the rest of the server uses.
//!
//! There is one clock and one parser here. Everything that needs "now" as
//! milliseconds since the epoch calls [`now_ms`], and everything that reads a
//! stored stamp back calls [`parse_iso_ms`] — the exact inverse of the
//! [`iso8601`](crate::sessions::iso8601) writer, with its leniency kept as a
//! contract: a `T` or a space between date and time, an optional fractional
//! part of any length, and an optional `Z` (with anything after the seconds
//! ignored).
//!
//! [`parse_iso`] is the stricter reader for a stamp this server did not write:
//! a hand-editable schedule file may hold any RFC 3339 string, and a stamp that
//! does not parse reads as "no schedule".

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
///
/// A clock that reads before the epoch is clamped to zero.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or_default()
}

/// A stored ISO-8601 stamp as an instant, if it is one.
///
/// Only [`crate::sessions::iso8601`]'s own output is ever parsed here, but a
/// schedule file is editable by hand and a stamp that does not parse must not
/// wedge the runner — it reads as "no schedule", which the next tick fixes by
/// computing one.
pub fn parse_iso(stamp: &str) -> Option<SystemTime> {
    chrono::DateTime::parse_from_rfc3339(stamp).ok().map(SystemTime::from)
}

/// Milliseconds since the epoch, for a stamp [`crate::sessions::iso8601`] wrote.
///
/// The field scan is deliberately lenient — a `T` or a space, any fraction, an
/// optional `Z` — because a recording made by an older writer must keep
/// replaying. Only the civil-date arithmetic is chrono's.
pub fn parse_iso_ms(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || (bytes[10] != b'T' && bytes[10] != b' ') {
        return None;
    }
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let millis = match text.get(19..20) {
        Some(".") => {
            let digits: String = text[20..].chars().take_while(char::is_ascii_digit).collect();
            // `.5` is half a second, `.5000` is still half a second: pad rather
            // than parse, so a stamp from another writer reads correctly.
            let padded = format!("{digits:0<3}");
            padded.get(..3)?.parse::<i64>().ok()?
        }
        _ => 0,
    };

    // The 1st of the month, plus the day offset: the hand-rolled days_from_civil
    // normalised a day past the month's end the same way, and the rest of the
    // timestamp is plain arithmetic so the caller keeps every value it accepted.
    let first = chrono::NaiveDate::from_ymd_opt(year as i32, month as u32, 1)?;
    let date = first.checked_add_days(chrono::Days::new((day - 1) as u64))?;
    let midnight = date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis();
    Some(midnight + hour * 3_600_000 + minute * 60_000 + second * 1_000 + millis)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_stamp_keeps_the_leniency_it_had() {
        for millis in [0i64, 1, 1_000, 1_700_000_000_123, 253_370_764_800_000] {
            let stamp = crate::sessions::iso8601(UNIX_EPOCH + Duration::from_millis(millis as u64));
            assert_eq!(parse_iso_ms(&stamp), Some(millis), "{stamp}");
        }
        assert_eq!(parse_iso_ms("2026-08-29 12:00:00"), parse_iso_ms("2026-08-29T12:00:00Z"));
        assert_eq!(parse_iso_ms("2026-08-29T12:00:00Z"), parse_iso_ms("2026-08-29T12:00:00.000Z"));
        assert_eq!(parse_iso_ms("2026-08-29T12:00:00.5Z"), parse_iso_ms("2026-08-29T12:00:00.500Z"));
        // A day past the month's end keeps normalising: the 31st of February
        // is the 3rd of March.
        assert_eq!(parse_iso_ms("2026-02-31T00:00:00"), parse_iso_ms("2026-03-03T00:00:00"));
        for bad in ["", "nonsense", "2026-08-29", "2026-13-01T00:00:00.000Z", "2026-08-32T00:00:00.000Z"] {
            assert_eq!(parse_iso_ms(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_schedule_stamp_reads_only_when_it_is_rfc3339() {
        assert!(parse_iso("2026-08-29T12:00:00.000Z").is_some());
        assert!(parse_iso("2026-08-29 12:00:00").is_none());
        assert!(parse_iso("nonsense").is_none());
    }
}
