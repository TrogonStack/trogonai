use std::fmt;
use std::time::SystemTime;

use time::{OffsetDateTime, Time, UtcOffset, Weekday};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeOfDayWindow {
    weekdays: WeekdaySet,
    start: Time,
    end: Time,
    offset: UtcOffset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WeekdaySet {
    All,
    Range { start: Weekday, end: Weekday },
    Explicit(Vec<Weekday>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeOfDayParseError {
    Empty,
    SegmentCount { expected: usize, got: usize },
    InvalidWeekdays(String),
    InvalidTimeWindow(String),
    InvalidTimezone(String),
}

impl fmt::Display for TimeOfDayParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "time-of-day pattern must not be empty"),
            Self::SegmentCount { expected, got } => {
                write!(f, "time-of-day pattern expected {expected} segments, got {got}")
            }
            Self::InvalidWeekdays(msg) => write!(f, "invalid weekdays: {msg}"),
            Self::InvalidTimeWindow(msg) => write!(f, "invalid time window: {msg}"),
            Self::InvalidTimezone(msg) => write!(f, "invalid timezone: {msg}"),
        }
    }
}

impl std::error::Error for TimeOfDayParseError {}

impl TimeOfDayWindow {
    pub fn parse(pattern: &str) -> Result<Self, TimeOfDayParseError> {
        let trimmed = pattern.trim();
        if trimmed.is_empty() {
            return Err(TimeOfDayParseError::Empty);
        }
        let segments: Vec<&str> = trimmed.split('|').map(str::trim).collect();
        if segments.len() != 3 {
            return Err(TimeOfDayParseError::SegmentCount {
                expected: 3,
                got: segments.len(),
            });
        }
        Ok(Self {
            weekdays: parse_weekdays(segments[0])?,
            start: parse_window_bound(segments[1], true)?,
            end: parse_window_bound(segments[1], false)?,
            offset: parse_timezone(segments[2])?,
        })
    }

    pub fn contains(&self, instant: SystemTime) -> bool {
        let local = OffsetDateTime::from(instant).to_offset(self.offset);
        if !self.weekdays.contains(local.weekday()) {
            return false;
        }
        let clock = local.time();
        clock >= self.start && clock < self.end
    }
}

impl WeekdaySet {
    fn contains(&self, weekday: Weekday) -> bool {
        match self {
            Self::All => true,
            Self::Range { start, end } => weekday_in_range(weekday, *start, *end),
            Self::Explicit(days) => days.contains(&weekday),
        }
    }
}

fn weekday_in_range(day: Weekday, start: Weekday, end: Weekday) -> bool {
    let day = weekday_number(day);
    let start = weekday_number(start);
    let end = weekday_number(end);
    if start <= end {
        (start..=end).contains(&day)
    } else {
        day >= start || day <= end
    }
}

fn weekday_number(day: Weekday) -> u8 {
    match day {
        Weekday::Monday => 0,
        Weekday::Tuesday => 1,
        Weekday::Wednesday => 2,
        Weekday::Thursday => 3,
        Weekday::Friday => 4,
        Weekday::Saturday => 5,
        Weekday::Sunday => 6,
    }
}

fn parse_weekdays(raw: &str) -> Result<WeekdaySet, TimeOfDayParseError> {
    if raw == "*" {
        return Ok(WeekdaySet::All);
    }
    if let Some((start, end)) = raw.split_once('-') {
        return Ok(WeekdaySet::Range {
            start: parse_weekday_token(start)?,
            end: parse_weekday_token(end)?,
        });
    }
    let mut days = Vec::new();
    for token in raw.split(',') {
        days.push(parse_weekday_token(token)?);
    }
    if days.is_empty() {
        return Err(TimeOfDayParseError::InvalidWeekdays(raw.into()));
    }
    Ok(WeekdaySet::Explicit(days))
}

fn parse_weekday_token(token: &str) -> Result<Weekday, TimeOfDayParseError> {
    match token.trim() {
        "Mon" => Ok(Weekday::Monday),
        "Tue" => Ok(Weekday::Tuesday),
        "Wed" => Ok(Weekday::Wednesday),
        "Thu" => Ok(Weekday::Thursday),
        "Fri" => Ok(Weekday::Friday),
        "Sat" => Ok(Weekday::Saturday),
        "Sun" => Ok(Weekday::Sunday),
        other => Err(TimeOfDayParseError::InvalidWeekdays(other.into())),
    }
}

fn parse_window_bound(raw: &str, start: bool) -> Result<Time, TimeOfDayParseError> {
    let (lhs, rhs) = raw
        .split_once('-')
        .ok_or_else(|| TimeOfDayParseError::InvalidTimeWindow(raw.into()))?;
    let token = if start { lhs } else { rhs };
    parse_hh_mm(token.trim()).map_err(|_| TimeOfDayParseError::InvalidTimeWindow(raw.into()))
}

fn parse_hh_mm(raw: &str) -> Result<Time, ()> {
    let (hour, minute) = raw
        .split_once(':')
        .ok_or(())?;
    let hour: u8 = hour.parse().map_err(|_| ())?;
    let minute: u8 = minute.parse().map_err(|_| ())?;
    Time::from_hms(hour, minute, 0).map_err(|_| ())
}

fn parse_timezone(raw: &str) -> Result<UtcOffset, TimeOfDayParseError> {
    let raw = raw.trim();
    if raw.eq_ignore_ascii_case("utc") || raw == "Z" {
        return Ok(UtcOffset::UTC);
    }
    if let Some(rest) = raw.strip_prefix('+').or_else(|| raw.strip_prefix('-')) {
        let sign = if raw.starts_with('-') { -1 } else { 1 };
        let (hours, minutes) = if let Some((h, m)) = rest.split_once(':') {
            (h, m)
        } else {
            (rest, "00")
        };
        let hours: i8 = hours.parse().map_err(|_| TimeOfDayParseError::InvalidTimezone(raw.into()))?;
        let minutes: i8 = minutes.parse().map_err(|_| TimeOfDayParseError::InvalidTimezone(raw.into()))?;
        let total_seconds = sign * (i32::from(hours) * 3_600 + i32::from(minutes) * 60);
        return UtcOffset::from_whole_seconds(total_seconds)
            .map_err(|_| TimeOfDayParseError::InvalidTimezone(raw.into()));
    }
    Err(TimeOfDayParseError::InvalidTimezone(format!(
        "{raw} (use UTC, Z, or ±HH:MM offset)"
    )))
}

pub fn time_of_day_pattern_matches(pattern: &str, instant: SystemTime) -> Result<bool, TimeOfDayParseError> {
    TimeOfDayWindow::parse(pattern).map(|window| window.contains(instant))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    fn instant_utc(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> SystemTime {
        let datetime = time::Date::from_calendar_date(year, time::Month::try_from(month).unwrap(), day)
            .unwrap()
            .with_hms(hour, minute, 0)
            .unwrap()
            .assume_utc();
        UNIX_EPOCH + Duration::from_secs(datetime.unix_timestamp() as u64)
    }

    #[test]
    fn business_hours_window_matches_inside_weekday() {
        let window = TimeOfDayWindow::parse("Mon-Fri|09:00-17:00|UTC").unwrap();
        let inside = instant_utc(2026, 5, 25, 10, 0);
        assert!(window.contains(inside));
        let outside = instant_utc(2026, 5, 25, 18, 0);
        assert!(!window.contains(outside));
        let weekend = instant_utc(2026, 5, 24, 10, 0);
        assert!(!window.contains(weekend));
    }

    #[test]
    fn pattern_helper_matches() {
        let inside = instant_utc(2026, 5, 25, 12, 0);
        assert!(time_of_day_pattern_matches("Mon-Fri|09:00-17:00|UTC", inside).unwrap());
    }
}
