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

#[test]
fn empty_pattern_returns_typed_error() {
    assert!(matches!(TimeOfDayWindow::parse("   "), Err(TimeOfDayParseError::Empty)));
}

#[test]
fn segment_count_mismatch_returns_typed_error() {
    let err = TimeOfDayWindow::parse("Mon-Fri|09:00-17:00").unwrap_err();
    assert!(matches!(err, TimeOfDayParseError::SegmentCount { expected: 3, got: 2 }));
}

#[test]
fn wildcard_weekdays_match_every_day() {
    let window = TimeOfDayWindow::parse("*|00:00-23:59|UTC").expect("wildcard");
    // Sunday at 12:00 — would be rejected by Mon-Fri range, but `*` covers it.
    let sunday = instant_utc(2026, 5, 24, 12, 0);
    assert!(window.contains(sunday));
}

#[test]
fn explicit_weekday_list_matches_listed_days_only() {
    let window = TimeOfDayWindow::parse("Sat,Sun|10:00-18:00|UTC").expect("explicit");
    let saturday = instant_utc(2026, 5, 23, 12, 0);
    let monday = instant_utc(2026, 5, 25, 12, 0);
    assert!(window.contains(saturday));
    assert!(!window.contains(monday));
}

#[test]
fn invalid_weekday_token_returns_typed_error() {
    let err = TimeOfDayWindow::parse("Funday|09:00-17:00|UTC").unwrap_err();
    assert!(matches!(err, TimeOfDayParseError::InvalidWeekdays(_)));
}

#[test]
fn invalid_time_window_returns_typed_error() {
    let err = TimeOfDayWindow::parse("Mon-Fri|nope|UTC").unwrap_err();
    assert!(matches!(err, TimeOfDayParseError::InvalidTimeWindow(_)));
}

#[test]
fn z_timezone_alias_resolves_to_utc() {
    let window = TimeOfDayWindow::parse("Mon-Fri|09:00-17:00|Z").expect("z alias");
    let inside = instant_utc(2026, 5, 25, 12, 0);
    assert!(window.contains(inside));
}

#[test]
fn positive_offset_timezone_parses() {
    let window = TimeOfDayWindow::parse("Mon-Fri|09:00-17:00|+05:30").expect("offset");
    // 09:00 IST is 03:30 UTC; an event at 03:31 UTC should match.
    let inside = instant_utc(2026, 5, 25, 3, 31);
    assert!(window.contains(inside));
}

#[test]
fn negative_offset_timezone_parses() {
    let window = TimeOfDayWindow::parse("Mon-Fri|09:00-17:00|-05").expect("offset");
    // 09:00 EST (= -05) is 14:00 UTC.
    let inside = instant_utc(2026, 5, 25, 14, 5);
    assert!(window.contains(inside));
}

#[test]
fn invalid_timezone_returns_typed_error() {
    let err = TimeOfDayWindow::parse("Mon-Fri|09:00-17:00|EST").unwrap_err();
    assert!(matches!(err, TimeOfDayParseError::InvalidTimezone(_)));
}

#[test]
fn wraparound_weekday_range_includes_weekend_to_weekday_span() {
    // Sun-Tue should accept Sunday and Monday but reject Wednesday — the
    // wrap-around branch in `weekday_in_range` matters when the range
    // crosses Sunday.
    let window = TimeOfDayWindow::parse("Sat-Tue|00:00-23:59|UTC").expect("wrap");
    let saturday = instant_utc(2026, 5, 23, 12, 0);
    let monday = instant_utc(2026, 5, 25, 12, 0);
    let wednesday = instant_utc(2026, 5, 27, 12, 0);
    assert!(window.contains(saturday));
    assert!(window.contains(monday));
    assert!(!window.contains(wednesday));
}
