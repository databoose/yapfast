//! Message times and days in the local time zone.

use chrono::{DateTime, Local, TimeZone};

/// Now, in Unix milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The clock time of a Unix time, such as `14:05`.
pub fn time(seconds: u64) -> String {
    time_on(seconds, false)
}

/// The clock time of a Unix time on a 24-hour clock, such as `21:41`, or a 12-hour one,
/// such as `9:41 PM`.
pub fn time_on(seconds: u64, twelve_hour: bool) -> String {
    time_in(&Local, seconds, twelve_hour, false)
}

/// The same with its seconds, such as `21:41:05` or `9:41:05 PM`, when
/// `with_seconds`.
pub fn time_with(seconds: u64, twelve_hour: bool, with_seconds: bool) -> String {
    time_in(&Local, seconds, twelve_hour, with_seconds)
}

/// The calendar date of a Unix time, such as `3 October 2026`; empty when unknown.
pub fn day(seconds: u64) -> String {
    if seconds == 0 {
        return String::new();
    }
    day_in(&Local, seconds)
}

fn time_in<Tz: TimeZone>(zone: &Tz, seconds: u64, twelve_hour: bool, with_seconds: bool) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let format = match (twelve_hour, with_seconds) {
        (true, false) => "%-I:%M %p",
        (true, true) => "%-I:%M:%S %p",
        (false, false) => "%H:%M",
        (false, true) => "%H:%M:%S",
    };
    at(zone, seconds).format(format).to_string()
}

/// The local calendar date of a Unix time, which [`day`] names; none when unknown.
pub fn date(seconds: u64) -> Option<chrono::NaiveDate> {
    (seconds != 0).then(|| at(&Local, seconds).date_naive())
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
        assert_eq!(time_in(&utc, seconds, false, false), "23:30");
        assert_eq!(time_in(&utc, seconds, true, false), "11:30 PM");
        assert_eq!(time_in(&utc, seconds + 5, false, true), "23:30:05");
        assert_eq!(time_in(&utc, seconds + 5, true, true), "11:30:05 PM");
        assert_eq!(day_in(&utc, seconds), "6 October 2026");
        assert_eq!(time_in(&plus_two, seconds, false, false), "01:30");
        assert_eq!(time_in(&plus_two, seconds, true, false), "1:30 AM");
        assert_eq!(day_in(&plus_two, seconds), "7 October 2026");
        assert_eq!(day(0), "");
    }
}
