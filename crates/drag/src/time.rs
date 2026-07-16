//! Duration, interval, clock, and relative-date parsing.

use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone,
};
use chrono_tz::Tz;

use crate::{models::ClockInterval, Error};

/// Result of parsing either a duration (`1h15m`) or interval (`11-12:30`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDuration {
    pub seconds: i64,
    pub start_time: Option<NaiveTime>,
}

/// Date selected by a command and the default start time for duration input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedDate {
    pub date: NaiveDate,
    pub default_start_time: NaiveTime,
}

/// Parse a duration or clock interval using the supplied local date/time zone.
///
/// # Errors
/// Returns [`Error::InvalidDuration`] for unsupported syntax.
pub fn parse_duration_or_interval(
    input: &str,
    reference_date: NaiveDate,
    timezone: Tz,
) -> Result<ParsedDuration, Error> {
    if let Some(seconds) = parse_duration(input) {
        return Ok(ParsedDuration {
            seconds,
            start_time: None,
        });
    }

    let mut pieces = input.split('-');
    let start = pieces.next().and_then(parse_clock);
    let end = pieces.next().and_then(parse_clock);
    if pieces.next().is_some() || start.is_none() || end.is_none() {
        return Err(Error::InvalidDuration(input.to_owned()));
    }
    let start = start.ok_or_else(|| Error::InvalidDuration(input.to_owned()))?;
    let end = end.ok_or_else(|| Error::InvalidDuration(input.to_owned()))?;
    let start_at = resolve_local(reference_date, start, timezone);
    let end_at = resolve_local(reference_date, end, timezone);
    let difference = end_at.timestamp() - start_at.timestamp();
    let seconds = if end_at > start_at {
        difference
    } else {
        86_400 + difference
    };

    Ok(ParsedDuration {
        seconds,
        start_time: Some(start),
    })
}

/// Parse a supported clock form (`9`, `9:30`, `09.30`).
#[must_use]
pub fn parse_clock(input: &str) -> Option<NaiveTime> {
    let (hours, minutes) =
        if let Some((hours, minutes)) = input.split_once(':').or_else(|| input.split_once('.')) {
            if minutes.contains([':', '.']) {
                return None;
            }
            (hours.parse::<u32>().ok()?, minutes.parse::<u32>().ok()?)
        } else {
            if input.len() > 2 || input.is_empty() {
                return None;
            }
            (input.parse::<u32>().ok()?, 0)
        };
    NaiveTime::from_hms_opt(hours, minutes, 0)
}

/// Parse today/yesterday, a relative today selector, or an ISO date.
///
/// # Errors
/// Returns [`Error::InvalidDate`] when the selector is unsupported.
pub fn select_date(now: DateTime<Tz>, when: Option<&str>) -> Result<SelectedDate, Error> {
    let midnight = NaiveTime::MIN;
    let Some(when) = when else {
        return Ok(SelectedDate {
            date: now.date_naive(),
            default_start_time: now.time(),
        });
    };

    if matches!(when, "y" | "yesterday") {
        let date = now
            .date_naive()
            .checked_sub_signed(Duration::days(1))
            .ok_or_else(|| Error::InvalidDate(when.to_owned()))?;
        return Ok(SelectedDate {
            date,
            default_start_time: midnight,
        });
    }

    if let Some(days) = parse_today_offset(when) {
        let offset = Duration::try_days(days).ok_or_else(|| Error::InvalidDate(when.to_owned()))?;
        let date = now
            .date_naive()
            .checked_add_signed(offset)
            .ok_or_else(|| Error::InvalidDate(when.to_owned()))?;
        return Ok(SelectedDate {
            date,
            default_start_time: midnight,
        });
    }

    NaiveDate::parse_from_str(when, "%Y-%m-%d")
        .map(|date| SelectedDate {
            date,
            default_start_time: midnight,
        })
        .map_err(|_| Error::InvalidDate(when.to_owned()))
}

/// Convert seconds into Drag's compact duration form.
#[must_use]
pub fn format_duration(seconds: i64, plus_prefix: bool) -> String {
    let hours = seconds.unsigned_abs() / 3_600;
    let minutes = (seconds.unsigned_abs() % 3_600) / 60;
    if hours == 0 && minutes == 0 {
        return "0h".to_owned();
    }
    let mut result = String::new();
    if seconds < 0 {
        result.push('-');
    } else if seconds > 0 && plus_prefix {
        result.push('+');
    }
    if hours > 0 {
        result.push_str(&format!("{hours}h"));
    }
    if minutes > 0 {
        result.push_str(&format!("{minutes}m"));
    }
    result
}

/// Derive a clock interval from a Tempo start time and elapsed seconds.
#[must_use]
pub fn clock_interval(
    seconds: i64,
    start_time: &str,
    date: NaiveDate,
    timezone: Tz,
) -> Option<ClockInterval> {
    if seconds < 0 {
        return None;
    }
    let start = NaiveTime::parse_from_str(start_time, "%H:%M:%S").ok()?;
    let start_at = resolve_local(date, start, timezone);
    let end_at = start_at + Duration::seconds(seconds);
    Some(ClockInterval {
        start_time: start_at.format("%H:%M").to_string(),
        end_time: end_at.format("%H:%M").to_string(),
    })
}

/// Return the first and last date in the selected month.
#[must_use]
pub fn month_bounds(date: NaiveDate) -> (NaiveDate, NaiveDate) {
    let start = date.with_day(1).unwrap_or(date);
    let (year, month) = if date.month() == 12 {
        (date.year() + 1, 1)
    } else {
        (date.year(), date.month() + 1)
    };
    let next_month = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(start);
    (start, next_month - Duration::days(1))
}

fn parse_duration(input: &str) -> Option<i64> {
    let lower = input.to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }
    let (hours, minutes) = if let Some((hours, rest)) = lower.split_once('h') {
        if hours.is_empty() || !hours.chars().all(|character| character.is_ascii_digit()) {
            return None;
        }
        let hours = hours.parse::<i64>().ok()?;
        if rest.is_empty() {
            (hours, 0)
        } else {
            let minutes = rest.strip_suffix('m')?;
            if minutes.is_empty() || !minutes.chars().all(|character| character.is_ascii_digit()) {
                return None;
            }
            (hours, minutes.parse::<i64>().ok()?)
        }
    } else {
        let minutes = lower.strip_suffix('m')?;
        if minutes.is_empty() || !minutes.chars().all(|character| character.is_ascii_digit()) {
            return None;
        }
        (0, minutes.parse::<i64>().ok()?)
    };
    hours
        .checked_mul(3_600)?
        .checked_add(minutes.checked_mul(60)?)
}

fn parse_today_offset(input: &str) -> Option<i64> {
    let rest = input
        .strip_prefix("today")
        .or_else(|| input.strip_prefix('t'))?;
    if rest.len() < 2 || !matches!(rest.as_bytes().first(), Some(b'+') | Some(b'-')) {
        return None;
    }
    rest.parse().ok()
}

fn resolve_local(date: NaiveDate, time: NaiveTime, timezone: Tz) -> DateTime<Tz> {
    let local = NaiveDateTime::new(date, time);
    match timezone.from_local_datetime(&local) {
        LocalResult::Single(value) => value,
        LocalResult::Ambiguous(earliest, _) => earliest,
        LocalResult::None => {
            // Match JavaScript Date's DST-gap normalization by advancing one hour
            // while preserving minutes (02:59 becomes 03:59).
            let shifted = local + Duration::hours(1);
            match timezone.from_local_datetime(&shifted) {
                LocalResult::Single(value) | LocalResult::Ambiguous(value, _) => value,
                LocalResult::None => timezone.from_utc_datetime(&local),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use chrono_tz::Europe::Warsaw;

    use super::{clock_interval, format_duration, parse_duration_or_interval, select_date};
    use crate::Error;

    #[test]
    fn parses_original_duration_forms() {
        let date = Warsaw.with_ymd_and_hms(2020, 1, 1, 12, 0, 0).single();
        let Some(date) = date else { return };
        for (input, expected) in [
            ("1h", 3_600),
            ("100m", 6_000),
            ("1h15m", 4_500),
            ("11-13:00", 7_200),
            ("23:50-00:10", 1_200),
            ("12-12", 86_400),
        ] {
            let parsed = parse_duration_or_interval(input, date.date_naive(), Warsaw);
            assert_eq!(parsed.map(|value| value.seconds), Ok(expected), "{input}");
        }
    }

    #[test]
    fn rejects_original_invalid_forms() {
        let date = chrono::NaiveDate::from_ymd_opt(2020, 1, 1);
        let Some(date) = date else { return };
        for input in ["", "5", "15m1h", "1100-1300", "22:00-30:00"] {
            assert!(
                parse_duration_or_interval(input, date, Warsaw).is_err(),
                "{input}"
            );
        }
    }

    #[test]
    fn preserves_dst_elapsed_time_behavior() {
        let spring = chrono::NaiveDate::from_ymd_opt(2020, 3, 29);
        let autumn = chrono::NaiveDate::from_ymd_opt(2020, 10, 25);
        let (Some(spring), Some(autumn)) = (spring, autumn) else {
            return;
        };
        assert_eq!(
            parse_duration_or_interval("00:00-5", spring, Warsaw).map(|value| value.seconds),
            Ok(14_400)
        );
        assert_eq!(
            parse_duration_or_interval("00:00-5", autumn, Warsaw).map(|value| value.seconds),
            Ok(21_600)
        );
    }

    #[test]
    fn formats_durations_and_intervals() {
        assert_eq!(format_duration(4_500, false), "1h15m");
        assert_eq!(format_duration(-4_500, false), "-1h15m");
        assert_eq!(format_duration(60, true), "+1m");
        let date = chrono::NaiveDate::from_ymd_opt(2020, 1, 1);
        let Some(date) = date else { return };
        let interval = clock_interval(3_600, "23:30:00", date, Warsaw);
        assert_eq!(
            interval.map(|value| value.end_time),
            Some("00:30".to_owned())
        );
    }

    #[test]
    fn parses_relative_dates() {
        let now = Warsaw.with_ymd_and_hms(2020, 2, 28, 12, 0, 0).single();
        let Some(now) = now else { return };
        assert_eq!(
            select_date(now, Some("y")).map(|value| value.date.to_string()),
            Ok("2020-02-27".to_owned())
        );
        assert_eq!(
            select_date(now, Some("today+10")).map(|value| value.date.to_string()),
            Ok("2020-03-09".to_owned())
        );
        for selector in [
            "today+9223372036854775807",
            "today-9223372036854775808",
            "today+999999999",
        ] {
            assert!(matches!(
                select_date(now, Some(selector)),
                Err(Error::InvalidDate(value)) if value == selector
            ));
        }
    }
}
