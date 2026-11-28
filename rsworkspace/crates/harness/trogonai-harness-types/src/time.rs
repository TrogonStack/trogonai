//! Typed timestamps and elapsed durations.

use std::borrow::Cow;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::local_time::LocalReading;

/// A moment, in milliseconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UnixMillis(i64);

impl UnixMillis {
    /// Takes a millisecond count as it appears in a session file.
    #[must_use]
    pub const fn new(millis: i64) -> Self {
        Self(millis)
    }

    /// Reads a persisted millisecond count, treating `0` as no moment.
    #[must_use]
    pub const fn from_persisted(millis: i64) -> Option<Self> {
        if millis == 0 { None } else { Some(Self(millis)) }
    }

    /// The moment as a millisecond count.
    #[must_use]
    pub const fn as_millis(self) -> i64 {
        self.0
    }

    /// Reads a host clock's answer.
    ///
    /// Answers `None` for a time before the epoch, which a host clock only
    /// reports when the machine's own clock is wrong.
    #[must_use]
    pub fn from_system_time(time: SystemTime) -> Option<Self> {
        let millis = time.duration_since(UNIX_EPOCH).ok()?.as_millis();
        i64::try_from(millis).ok().map(Self)
    }

    /// How long after `earlier` this moment is, or `None` if it is not after.
    #[must_use]
    pub fn since(self, earlier: Self) -> Option<Duration> {
        let elapsed = self.0.checked_sub(earlier.0)?;
        u64::try_from(elapsed).ok().map(Duration::from_millis)
    }

    /// The moment `span` later, or `None` if there is no such moment.
    #[must_use]
    pub fn plus(self, span: Duration) -> Option<Self> {
        let millis = i64::try_from(span.as_millis()).ok()?;
        self.0.checked_add(millis).map(Self)
    }
}

/// A date on the proleptic Gregorian calendar.
///
/// The turn context carries one so a model reasoning about release dates, changelog entries or
/// "last week" has something to reason from; without it, it answers from whenever it was
/// trained. A usage report names its window with one too, which is why the type sits here
/// rather than beside either caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CivilDate {
    /// The year, which may be negative on the proleptic calendar.
    pub year: i64,
    /// The month, 1 through 12.
    pub month: u32,
    /// The day of the month, starting at 1.
    pub day: u32,
}

const MS_PER_DAY: i64 = 86_400_000;

impl CivilDate {
    /// The UTC date at a Unix millisecond timestamp.
    #[must_use]
    pub const fn from_unix_millis(epoch_ms: i64) -> Self {
        Self::from_unix_days(epoch_ms.div_euclid(MS_PER_DAY))
    }

    /// The UTC date a whole number of days from the epoch.
    ///
    /// Every division floors, which is what makes it work for days before the epoch as well as
    /// after.
    #[must_use]
    pub const fn from_unix_days(days_since_epoch: i64) -> Self {
        let z = days_since_epoch + 719_468;
        let era = z.div_euclid(146_097);
        let day_of_era = z - era * 146_097;
        let year_of_era = (day_of_era - day_of_era.div_euclid(1_460) + day_of_era.div_euclid(36_524)
            - day_of_era.div_euclid(146_096))
        .div_euclid(365);
        let mut year = year_of_era + era * 400;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era.div_euclid(4) - year_of_era.div_euclid(100));
        let month_position = (5 * day_of_year + 2).div_euclid(153);
        let day = day_of_year - (153 * month_position + 2).div_euclid(5) + 1;
        let month = month_position + if month_position < 10 { 3 } else { -9 };
        if month <= 2 {
            year += 1;
        }
        Self {
            year,
            month: month as u32,
            day: day as u32,
        }
    }
}

impl fmt::Display for CivilDate {
    /// Writes the date the way the turn context carries it.
    ///
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// Formats a millisecond timestamp the way a session listing prints it, in the
/// local time zone with its label.
///
/// Out-of-range values answer `"unknown"` rather than guessing.
#[must_use]
pub fn format_session_list_timestamp(millis: i64) -> Cow<'static, str> {
    match LocalReading::at(millis) {
        Some(reading) => Cow::Owned(reading.to_string()),
        None => Cow::Borrowed("unknown"),
    }
}

/// Seconds in a day.
const SECS_PER_DAY: i64 = 86_400;

/// A moment on the proleptic Gregorian calendar, counted to the second, in UTC.
///
/// It sits beside [`CivilDate`] rather than in the tool because the conversion is the same
/// arithmetic, and because a second caller wanting a timestamp should find one instead of
/// writing the leap-year rules again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CivilDateTime {
    /// The date the moment falls on.
    pub date: CivilDate,
    /// The hour of the day, 0 through 23.
    pub hour: u32,
    /// The minute of the hour, 0 through 59.
    pub minute: u32,
    /// The second of the minute, 0 through 59.
    pub second: u32,
}

impl CivilDateTime {
    /// The UTC moment a whole number of seconds after the epoch.
    ///
    /// Every division floors, so a moment before the epoch lands on the day
    /// before it with a positive time of day rather than a negative one.
    #[must_use]
    pub const fn from_unix_seconds(seconds: i64) -> Self {
        let day = seconds.div_euclid(SECS_PER_DAY);
        let within_day = seconds.rem_euclid(SECS_PER_DAY);
        Self {
            date: CivilDate::from_unix_days(day),
            hour: (within_day / 3_600) as u32,
            minute: (within_day % 3_600 / 60) as u32,
            second: (within_day % 60) as u32,
        }
    }

    /// The UTC moment at a millisecond timestamp, with the milliseconds dropped.
    #[must_use]
    pub const fn from_unix_millis(at: UnixMillis) -> Self {
        Self::from_unix_seconds(at.as_millis().div_euclid(1_000))
    }
}

impl fmt::Display for CivilDateTime {
    /// Writes the moment the way `file_info` reports a modification time.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}T{:02}:{:02}:{:02}Z",
            self.date, self.hour, self.minute, self.second
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_time::{MAX_TIMESTAMP_MS, Zone, pin_zone};

    #[test]
    fn the_references_zero_reads_as_no_moment_at_all() {
        assert_eq!(UnixMillis::from_persisted(0), None);
        assert_eq!(
            UnixMillis::from_persisted(1_700_000_000_000),
            Some(UnixMillis::new(1_700_000_000_000)),
        );
    }

    #[test]
    fn an_elapsed_span_is_a_duration_and_a_backwards_one_is_nothing() {
        let started = UnixMillis::new(1_000);
        let later = UnixMillis::new(4_500);

        assert_eq!(later.since(started), Some(Duration::from_millis(3_500)));
        assert_eq!(started.since(later), None);
        assert_eq!(started.since(started), Some(Duration::ZERO));
    }

    #[test]
    fn a_host_clocks_answer_becomes_a_moment() {
        let time = UNIX_EPOCH + Duration::from_millis(1_700_000_000_123);
        assert_eq!(
            UnixMillis::from_system_time(time),
            Some(UnixMillis::new(1_700_000_000_123)),
        );
        assert_eq!(UnixMillis::from_system_time(UNIX_EPOCH - Duration::from_secs(1)), None,);
    }

    #[test]
    fn the_epoch_is_the_first_day_of_nineteen_seventy() {
        assert_eq!(CivilDate::from_unix_millis(0).to_string(), "1970-01-01");
    }

    #[test]
    fn a_time_within_a_day_stays_on_that_day() {
        assert_eq!(CivilDate::from_unix_millis(MS_PER_DAY - 1).to_string(), "1970-01-01");
        assert_eq!(CivilDate::from_unix_millis(MS_PER_DAY).to_string(), "1970-01-02");
    }

    #[test]
    fn a_time_before_the_epoch_lands_on_the_day_before() {
        assert_eq!(CivilDate::from_unix_millis(-1).to_string(), "1969-12-31");
        assert_eq!(CivilDate::from_unix_millis(-MS_PER_DAY).to_string(), "1969-12-31");
    }

    #[test]
    fn leap_days_are_days() {
        assert_eq!(
            CivilDate::from_unix_days(11_016).to_string(),
            "2000-02-29",
            "2000 is a leap year because it divides by 400"
        );
        assert_eq!(CivilDate::from_unix_days(19_782).to_string(), "2024-02-29");
    }

    #[test]
    fn known_timestamps_land_on_their_known_dates() {
        assert_eq!(CivilDate::from_unix_millis(1_700_000_000_000).to_string(), "2023-11-14");
        assert_eq!(CivilDate::from_unix_millis(946_684_800_000).to_string(), "2000-01-01");
    }

    #[test]
    fn a_moment_carries_the_time_of_day_the_date_alone_drops() {
        assert_eq!(CivilDateTime::from_unix_seconds(0).to_string(), "1970-01-01T00:00:00Z");
        assert_eq!(
            CivilDateTime::from_unix_seconds(1_622_924_906).to_string(),
            "2021-06-05T20:28:26Z"
        );
        assert_eq!(
            CivilDateTime::from_unix_seconds(SECS_PER_DAY - 1).to_string(),
            "1970-01-01T23:59:59Z"
        );
    }

    #[test]
    fn a_moment_before_the_epoch_counts_forward_from_the_day_before() {
        assert_eq!(CivilDateTime::from_unix_seconds(-1).to_string(), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn milliseconds_are_dropped_rather_than_rounded() {
        assert_eq!(
            CivilDateTime::from_unix_millis(UnixMillis::new(1_622_924_906_999)).to_string(),
            "2021-06-05T20:28:26Z"
        );
        assert_eq!(
            CivilDateTime::from_unix_millis(UnixMillis::new(-1)).to_string(),
            "1969-12-31T23:59:59Z"
        );
    }

    #[test]
    fn a_session_list_timestamp_includes_milliseconds_and_the_zone() {
        let _zone = pin_zone(Zone::utc());
        assert_eq!(format_session_list_timestamp(2), "1970-01-01 00:00:00.002 UTC");
        assert_eq!(
            format_session_list_timestamp(1_622_924_906_789),
            "2021-06-05 20:28:26.789 UTC"
        );
    }

    #[test]
    fn a_session_list_timestamp_outside_the_supported_range_is_unknown() {
        let _zone = pin_zone(Zone::utc());
        assert_eq!(format_session_list_timestamp(-1), "unknown");
        assert_eq!(format_session_list_timestamp(MAX_TIMESTAMP_MS + 1), "unknown");
    }

    #[test]
    fn every_day_of_a_year_is_reached_exactly_once() {
        let mut day = CivilDate::from_unix_days(0);
        for offset in 1..=800 {
            let next = CivilDate::from_unix_days(offset);
            assert!(next > day, "{next:?} should follow {day:?}");
            assert!((1..=12).contains(&next.month), "{next:?} has a real month");
            assert!((1..=31).contains(&next.day), "{next:?} has a real day");
            day = next;
        }
    }
}
