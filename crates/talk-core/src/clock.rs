//! Message times and days in the local time zone.

use chrono::{DateTime, Local, TimeZone};

/// The clock time of a Unix time, such as `14:05`.
pub fn time(seconds: u64) -> String {
    time_in(&Local, seconds)
}

/// The calendar date of a Unix time, such as `3 October 2026`; empty when unknown.
pub fn day(seconds: u64) -> String {
    if seconds == 0 {
        return String::new();
    }
    day_in(&Local, seconds)
}

fn time_in<Tz: TimeZone>(zone: &Tz, seconds: u64) -> String
where
    Tz::Offset: std::fmt::Display,
{
    at(zone, seconds).format("%H:%M").to_string()
}

fn day_in<Tz: TimeZone>(zone: &Tz, seconds: u64) -> String
where
    Tz::Offset: std::fmt::Display,
{
    at(zone, seconds).format("%-d %B %Y").to_string()
}

fn at<Tz: TimeZone>(zone: &Tz, seconds: u64) -> DateTime<Tz> {
    let utc =
        DateTime::from_timestamp(i64::try_from(seconds).unwrap_or(i64::MAX), 0).unwrap_or_default();
    utc.with_timezone(zone)
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;

    #[test]
    fn times_and_days_follow_the_time_zone() {
        // 2026-10-06 23:30 UTC.
        let seconds = 1_791_329_400;
        let utc = FixedOffset::east_opt(0).unwrap();
        let plus_two = FixedOffset::east_opt(2 * 3600).unwrap();
        assert_eq!(time_in(&utc, seconds), "23:30");
        assert_eq!(day_in(&utc, seconds), "6 October 2026");
        assert_eq!(time_in(&plus_two, seconds), "01:30");
        assert_eq!(day_in(&plus_two, seconds), "7 October 2026");
        assert_eq!(day(0), "");
    }
}
