//! Wall-clock readings of timestamps, for text a person reads.
//!
//! The runtime stores and serializes times in UTC. Text meant for a person reads them
//! in that person's time zone instead, labelled with the zone's abbreviation so
//! a log line or a bug report still says which clock it was.

use std::cell::RefCell;
use std::fmt;

use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::time::{CivilDate, CivilDateTime};

/// 9999-12-31 23:59:59.999 UTC, the last moment a four-digit year can show.
pub const MAX_TIMESTAMP_MS: i64 = 253_402_300_799_999;

const MAX_ZONE_LABEL_LEN: usize = 10;
const SECS_PER_DAY: i64 = 86_400;

/// A zone abbreviation that is safe to put on a terminal.
///
/// `TZ` comes from the environment, so an abbreviation is only shown when it is
/// a short plain token. Anything else falls back to the offset in the `+HHMM`
/// form tzdata uses for zones without a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneLabel(String);

impl ZoneLabel {
    const UTC: &'static str = "UTC";

    fn from_abbreviation(abbreviation: &str, offset_s: i32) -> Self {
        if Self::is_plain(abbreviation) {
            return Self(abbreviation.to_owned());
        }
        let minutes = offset_s.unsigned_abs() / 60;
        let sign = if offset_s < 0 { '-' } else { '+' };
        Self(format!("{sign}{:02}{:02}", minutes / 60, minutes % 60))
    }

    fn is_plain(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= MAX_ZONE_LABEL_LEN
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'-')
    }

    /// The label, such as `PDT` or `+0545`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ZoneLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A time zone timestamps can be read in, with the rules that decide its
/// daylight saving time.
#[derive(Debug, Clone, PartialEq)]
pub struct Zone(TimeZone);

impl Zone {
    /// Coordinated Universal Time.
    #[must_use]
    pub fn utc() -> Self {
        Self(TimeZone::UTC)
    }

    /// The zone `TZ` names, or the system's when `TZ` is unset. Falls back to
    /// UTC where a platform has no zone to report.
    #[must_use]
    pub fn system() -> Self {
        Self(TimeZone::system())
    }

    /// A zone from a POSIX `TZ` rule such as `EST5EDT,M3.2.0,M11.1.0`.
    #[must_use]
    pub fn posix(rule: &str) -> Option<Self> {
        TimeZone::posix(rule).ok().map(Self)
    }
}

thread_local! {
    static ACTIVE_ZONE: RefCell<Option<Zone>> = const { RefCell::new(None) };
}

/// Keeps a thread reading local time in a chosen zone until it is dropped.
#[derive(Debug)]
#[must_use = "the zone is only pinned while the guard lives"]
pub struct ZonePin(Option<Zone>);

impl Drop for ZonePin {
    fn drop(&mut self) {
        ACTIVE_ZONE.with(|active| *active.borrow_mut() = self.0.take());
    }
}

/// Makes this thread read local time in `zone` instead of the machine's, so
/// rendered text is the same wherever a test runs.
pub fn pin_zone(zone: Zone) -> ZonePin {
    ZonePin(ACTIVE_ZONE.with(|active| active.borrow_mut().replace(zone)))
}

/// A timestamp's calendar date and clock time in one zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalReading {
    date: CivilDate,
    hour: u32,
    minute: u32,
    second: u32,
    millisecond: u32,
    zone: ZoneLabel,
}

impl LocalReading {
    /// Reads `timestamp_ms` in the local zone in effect at that moment, so
    /// daylight saving time follows each timestamp's own date. `None` outside
    /// 1970 through 9999.
    #[must_use]
    pub fn at(timestamp_ms: i64) -> Option<Self> {
        match ACTIVE_ZONE.with(|active| active.borrow().clone()) {
            Some(zone) => Self::in_zone(timestamp_ms, &zone),
            None => Self::in_zone(timestamp_ms, &Zone::system()),
        }
    }

    /// Reads `timestamp_ms` in `zone`, or in UTC when that zone's offset is a
    /// day or more or the shifted time would leave 1970 through 9999. `None`
    /// outside that range.
    #[must_use]
    pub fn in_zone(timestamp_ms: i64, zone: &Zone) -> Option<Self> {
        if !(0..=MAX_TIMESTAMP_MS).contains(&timestamp_ms) {
            return None;
        }
        let (offset_s, abbreviation) = match Timestamp::from_millisecond(timestamp_ms) {
            Ok(at) => {
                let info = zone.0.to_offset_info(at);
                (info.offset().seconds(), info.abbreviation().to_owned())
            }
            Err(_) => (0, ZoneLabel::UTC.to_owned()),
        };
        let shifted_ms = timestamp_ms + i64::from(offset_s) * 1_000;
        let (offset_s, abbreviation, local_ms) =
            if i64::from(offset_s).abs() >= SECS_PER_DAY || !(0..=MAX_TIMESTAMP_MS).contains(&shifted_ms) {
                (0, ZoneLabel::UTC.to_owned(), timestamp_ms)
            } else {
                (offset_s, abbreviation, shifted_ms)
            };
        let civil = CivilDateTime::from_unix_seconds(local_ms.div_euclid(1_000));
        Some(Self {
            date: civil.date,
            hour: civil.hour,
            minute: civil.minute,
            second: civil.second,
            millisecond: local_ms.rem_euclid(1_000) as u32,
            zone: ZoneLabel::from_abbreviation(&abbreviation, offset_s),
        })
    }

    /// The calendar date in the zone.
    #[must_use]
    pub const fn date(&self) -> CivilDate {
        self.date
    }

    /// The hour of the day, 0 through 23.
    #[must_use]
    pub const fn hour(&self) -> u32 {
        self.hour
    }

    /// The minute of the hour, 0 through 59.
    #[must_use]
    pub const fn minute(&self) -> u32 {
        self.minute
    }

    /// The zone's label.
    #[must_use]
    pub const fn zone(&self) -> &ZoneLabel {
        &self.zone
    }
}

impl fmt::Display for LocalReading {
    /// `2026-10-16 13:31:49.123 PDT`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {:02}:{:02}:{:02}.{:03} {}",
            self.date, self.hour, self.minute, self.second, self.millisecond, self.zone
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JULY_2024: i64 = 1_720_000_000_000;
    const NOVEMBER_2023: i64 = 1_700_000_060_000;

    fn zone(rule: &str) -> Zone {
        Zone::posix(rule).expect("a valid POSIX rule")
    }

    #[test]
    fn utc_reads_the_timestamp_as_it_is() {
        let reading = LocalReading::in_zone(1_622_924_906_789, &Zone::utc()).unwrap();
        assert_eq!(reading.to_string(), "2021-06-05 20:28:26.789 UTC");
    }

    #[test]
    fn a_zone_east_of_utc_moves_the_date_forward() {
        let reading = LocalReading::in_zone(NOVEMBER_2023, &zone("IST-5:30")).unwrap();
        assert_eq!(reading.to_string(), "2023-11-15 03:44:20.000 IST");
    }

    #[test]
    fn daylight_saving_follows_each_timestamps_own_date() {
        let rule = zone("EST5EDT,M3.2.0,M11.1.0");
        assert_eq!(
            LocalReading::in_zone(JULY_2024, &rule).unwrap().to_string(),
            "2024-07-03 05:46:40.000 EDT"
        );
        assert_eq!(
            LocalReading::in_zone(NOVEMBER_2023, &rule).unwrap().to_string(),
            "2023-11-14 17:14:20.000 EST"
        );
    }

    #[test]
    fn a_timestamp_outside_1970_through_9999_has_no_reading() {
        let utc = Zone::utc();
        assert_eq!(LocalReading::in_zone(-1, &utc), None);
        assert_eq!(LocalReading::in_zone(MAX_TIMESTAMP_MS + 1, &utc), None);
        assert!(LocalReading::in_zone(MAX_TIMESTAMP_MS, &utc).is_some());
    }

    #[test]
    fn a_shift_out_of_range_falls_back_to_utc() {
        let west = zone("MST7");
        let reading = LocalReading::in_zone(1, &west).unwrap();
        assert_eq!(reading.to_string(), "1970-01-01 00:00:00.001 UTC");
        let east = zone("JST-9");
        let reading = LocalReading::in_zone(MAX_TIMESTAMP_MS, &east).unwrap();
        assert_eq!(reading.to_string(), "9999-12-31 23:59:59.999 UTC");
    }

    #[test]
    fn an_abbreviation_that_is_not_a_plain_token_becomes_its_offset() {
        assert_eq!(ZoneLabel::from_abbreviation("", 20_700).as_str(), "+0545");
        assert_eq!(ZoneLabel::from_abbreviation("\u{1b}[2J", -3_600).as_str(), "-0100");
        assert_eq!(ZoneLabel::from_abbreviation("ABCDEFGHIJK", 0).as_str(), "+0000");
        assert_eq!(ZoneLabel::from_abbreviation("PDT", -25_200).as_str(), "PDT");
    }

    #[test]
    fn a_pinned_zone_replaces_the_machines_and_is_restored_afterwards() {
        let outer = pin_zone(Zone::utc());
        {
            let _inner = pin_zone(zone("IST-5:30"));
            assert_eq!(
                LocalReading::at(NOVEMBER_2023).unwrap().to_string(),
                "2023-11-15 03:44:20.000 IST"
            );
        }
        assert_eq!(
            LocalReading::at(NOVEMBER_2023).unwrap().to_string(),
            "2023-11-14 22:14:20.000 UTC"
        );
        drop(outer);
    }
}
