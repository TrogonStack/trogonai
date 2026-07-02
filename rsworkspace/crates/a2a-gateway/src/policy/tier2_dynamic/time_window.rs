use chrono::{DateTime, Datelike, NaiveTime, Utc};
use chrono_tz::Tz;

use super::day_of_week_set::DayOfWeekSet;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimeWindowError {
    #[error("time_window timezone `{0}` is not a recognized IANA timezone")]
    UnknownTimezone(Box<str>),
}

/// A recurring, timezone-aware time-of-day window, optionally restricted
/// to a subset of weekdays. Distinct from
/// [`crate::policy::tier1_declarative::time_predicate::TimeOfDayWindow`],
/// which is `time`-crate based and belongs to Tier-1 -- this is the
/// `chrono`/`chrono-tz` based Tier-2 dynamic-condition equivalent
/// described by the dynamic-policy-conditions spec's `time_window`
/// condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeWindow {
    timezone: Tz,
    start_time: NaiveTime,
    end_time: NaiveTime,
    days_of_week: Option<DayOfWeekSet>,
}

impl TimeWindow {
    pub fn new(
        timezone_name: &str,
        start_time: NaiveTime,
        end_time: NaiveTime,
        days_of_week: Option<DayOfWeekSet>,
    ) -> Result<Self, TimeWindowError> {
        let timezone: Tz = timezone_name
            .parse()
            .map_err(|_| TimeWindowError::UnknownTimezone(timezone_name.into()))?;
        Ok(Self {
            timezone,
            start_time,
            end_time,
            days_of_week,
        })
    }

    /// Whether `instant` falls inside this window, evaluated in the
    /// window's own timezone. `start_time` is inclusive and `end_time` is
    /// exclusive, per the dynamic-policy-conditions spec. A window whose
    /// `end_time` is earlier than `start_time` (e.g. `22:00`..`06:00`) is
    /// treated as wrapping past midnight.
    pub fn contains(&self, instant: DateTime<Utc>) -> bool {
        let local = instant.with_timezone(&self.timezone);
        if let Some(days) = &self.days_of_week
            && !days.contains(local.date_naive().weekday())
        {
            return false;
        }
        let time_of_day = local.time();
        if self.start_time <= self.end_time {
            time_of_day >= self.start_time && time_of_day < self.end_time
        } else {
            time_of_day >= self.start_time || time_of_day < self.end_time
        }
    }
}
