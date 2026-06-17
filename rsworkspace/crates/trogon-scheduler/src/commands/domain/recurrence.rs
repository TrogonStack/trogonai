use std::str::FromStr;

use chrono::{DateTime, Utc};
use trogonai_proto::convert::{TimestampConversionError, datetime_from_timestamp};
use trogonai_proto::google::r#type::TimeZone;
use trogonai_proto::scheduler::schedules::{ScheduleKind, v1};

/// Where the aggregate resumes recurrence expansion from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RRuleCursor {
    /// The next occurrence at or after this instant (inclusive boundary).
    AtOrAfter(DateTime<Utc>),
    /// The next occurrence strictly after this instant.
    After(DateTime<Utc>),
}

impl RRuleCursor {
    pub(crate) fn at_or_after(instant: DateTime<Utc>) -> Self {
        Self::AtOrAfter(instant)
    }

    pub(crate) fn after(instant: DateTime<Utc>) -> Self {
        Self::After(instant)
    }

    fn instant(self) -> DateTime<Utc> {
        match self {
            Self::AtOrAfter(instant) | Self::After(instant) => instant,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecurrenceError {
    #[error("schedule is not an RRULE schedule")]
    NotRRule,
    #[error("RRULE schedule is missing its {field}")]
    MissingField { field: &'static str },
    #[error("RRULE schedule {field} timestamp is invalid: {source}")]
    Timestamp {
        field: &'static str,
        #[source]
        source: TimestampConversionError,
    },
    #[error("RRULE schedule timezone '{id}' is not a known IANA timezone")]
    Timezone { id: String },
    #[error("RRULE set could not be expanded")]
    Expansion {
        #[source]
        source: rrule::RRuleError,
    },
}

/// The aggregate's decision about a schedule's next recurrence step.
///
/// This is the domain outcome the schedule commands translate into a concrete
/// schedule event; the domain never builds the proto event itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecurrenceStep {
    /// The recurrence continues at this instant.
    Occurrence { at: DateTime<Utc> },
    /// The recurrence has no further occurrences.
    Exhausted,
}

/// A parsed RRULE recurrence definition.
///
/// The schedule aggregate owns recurrence calculation. A [`Recurrence`] is built
/// from the persisted proto definition at the command boundary, then expanded
/// purely in domain terms so callers never expand an RRULE themselves.
#[derive(Debug, Clone)]
pub(crate) struct Recurrence {
    dtstart: DateTime<Utc>,
    rule: String,
    timezone: rrule::Tz,
    rdate: Vec<DateTime<Utc>>,
    exdate: Vec<DateTime<Utc>>,
}

impl TryFrom<&v1::Schedule> for Recurrence {
    type Error = RecurrenceError;

    fn try_from(schedule: &v1::Schedule) -> Result<Self, Self::Error> {
        let Some(ScheduleKind::Rrule(rrule)) = schedule.kind.as_ref() else {
            return Err(RecurrenceError::NotRRule);
        };

        let to_utc = |timestamp: &_, field: &'static str| {
            datetime_from_timestamp(timestamp).map_err(|source| RecurrenceError::Timestamp { field, source })
        };

        let timezone = rrule_timezone(rrule.timezone.as_option())?;
        let dtstart = to_utc(
            rrule
                .dtstart
                .as_option()
                .ok_or(RecurrenceError::MissingField { field: "dtstart" })?,
            "dtstart",
        )?;
        let rdate = rrule
            .rdate
            .iter()
            .map(|date| to_utc(date, "rdate"))
            .collect::<Result<Vec<_>, _>>()?;
        let exdate = rrule
            .exdate
            .iter()
            .map(|date| to_utc(date, "exdate"))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            dtstart,
            rule: rrule.rrule.clone(),
            timezone,
            rdate,
            exdate,
        })
    }
}

impl Recurrence {
    /// Decides the next recurrence step relative to `cursor`.
    pub(crate) fn plan_next(&self, cursor: RRuleCursor) -> Result<RecurrenceStep, RecurrenceError> {
        let tz = self.timezone;
        let mut set = rrule::RRuleSet::new(self.dtstart.with_timezone(&tz))
            .set_from_string(&format!("RRULE:{}", self.rule))
            .map_err(|source| RecurrenceError::Expansion { source })?;

        for date in &self.rdate {
            set = set.rdate(date.with_timezone(&tz));
        }
        for date in &self.exdate {
            set = set.exdate(date.with_timezone(&tz));
        }

        // `RRuleSet::after` is exclusive, so probe one nanosecond earlier for the
        // at-or-after cursor to keep an occurrence landing exactly on the cursor.
        let instant = cursor.instant();
        let probe = match cursor {
            RRuleCursor::AtOrAfter(_) => instant - chrono::Duration::nanoseconds(1),
            RRuleCursor::After(_) => instant,
        };
        let candidates = set.after(probe.with_timezone(&tz)).all(2).dates;
        let occurrence = candidates.into_iter().find(|date| match cursor {
            RRuleCursor::AtOrAfter(_) => date.with_timezone(&Utc) >= instant,
            RRuleCursor::After(_) => date.with_timezone(&Utc) > instant,
        });

        Ok(match occurrence {
            Some(date) => RecurrenceStep::Occurrence {
                at: date.with_timezone(&Utc),
            },
            None => RecurrenceStep::Exhausted,
        })
    }
}

fn rrule_timezone(timezone: Option<&TimeZone>) -> Result<rrule::Tz, RecurrenceError> {
    match timezone {
        None => Ok(rrule::Tz::UTC),
        Some(timezone) if timezone.id.is_empty() => Ok(rrule::Tz::UTC),
        Some(timezone) => {
            chrono_tz::Tz::from_str(&timezone.id)
                .map(Into::into)
                .map_err(|_| RecurrenceError::Timezone {
                    id: timezone.id.clone(),
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use buffa::MessageField;

    use super::*;
    use crate::commands::domain::{RRuleDateTime, RRuleExpression, RRuleTimezone, Schedule, ScheduleEventSchedule};

    fn instant(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw).unwrap().with_timezone(&Utc)
    }

    fn rrule_schedule(schedule: Schedule) -> v1::Schedule {
        v1::Schedule::try_from(&ScheduleEventSchedule::from(&schedule)).unwrap()
    }

    fn plan(schedule: &v1::Schedule, cursor: RRuleCursor) -> Result<RecurrenceStep, RecurrenceError> {
        Recurrence::try_from(schedule)?.plan_next(cursor)
    }

    fn occurrence(at: &str) -> RecurrenceStep {
        RecurrenceStep::Occurrence { at: instant(at) }
    }

    #[test]
    fn daily_rrule_returns_the_next_occurrence_at_or_after_the_cursor() {
        let schedule = rrule_schedule(Schedule::rrule("2026-01-01T00:00:00Z", "FREQ=DAILY;COUNT=3", None).unwrap());

        let next = plan(&schedule, RRuleCursor::at_or_after(instant("2026-01-01T12:00:00Z"))).unwrap();

        assert_eq!(next, occurrence("2026-01-02T00:00:00Z"));
    }

    #[test]
    fn at_or_after_cursor_includes_an_occurrence_exactly_on_the_cursor() {
        let schedule = rrule_schedule(Schedule::rrule("2026-01-01T00:00:00Z", "FREQ=DAILY;COUNT=3", None).unwrap());

        let next = plan(&schedule, RRuleCursor::at_or_after(instant("2026-01-01T00:00:00Z"))).unwrap();

        assert_eq!(next, occurrence("2026-01-01T00:00:00Z"));
    }

    #[test]
    fn strict_after_cursor_does_not_repeat_the_fired_occurrence() {
        let schedule = rrule_schedule(Schedule::rrule("2026-01-01T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap());

        let next = plan(&schedule, RRuleCursor::after(instant("2026-01-01T00:00:00Z"))).unwrap();

        assert_eq!(next, occurrence("2026-01-02T00:00:00Z"));
    }

    #[test]
    fn exhausted_rrule_is_reported() {
        let schedule = rrule_schedule(Schedule::rrule("2026-01-01T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap());

        let next = plan(&schedule, RRuleCursor::after(instant("2026-01-02T00:00:00Z"))).unwrap();

        assert_eq!(next, RecurrenceStep::Exhausted);
    }

    #[test]
    fn rdate_and_exdate_adjust_the_next_occurrence() {
        let schedule = rrule_schedule(Schedule::RRule {
            dtstart: RRuleDateTime::new("dtstart", "2026-01-01T00:00:00Z").unwrap(),
            rrule: RRuleExpression::new("FREQ=DAILY;COUNT=2").unwrap(),
            timezone: None,
            rdate: vec![RRuleDateTime::new("rdate", "2026-01-04T00:00:00Z").unwrap()],
            exdate: vec![RRuleDateTime::new("exdate", "2026-01-02T00:00:00Z").unwrap()],
        });

        let next = plan(&schedule, RRuleCursor::after(instant("2026-01-01T00:00:00Z"))).unwrap();

        assert_eq!(next, occurrence("2026-01-04T00:00:00Z"));
    }

    #[test]
    fn timezone_expansion_preserves_wall_clock_across_dst() {
        let schedule = rrule_schedule(Schedule::RRule {
            dtstart: RRuleDateTime::new("dtstart", "2026-03-07T14:00:00Z").unwrap(),
            rrule: RRuleExpression::new("FREQ=DAILY;COUNT=3").unwrap(),
            timezone: Some(RRuleTimezone::new("America/New_York").unwrap()),
            rdate: Vec::new(),
            exdate: Vec::new(),
        });

        let next = plan(&schedule, RRuleCursor::at_or_after(instant("2026-03-08T12:00:00Z"))).unwrap();

        assert_eq!(next, occurrence("2026-03-08T13:00:00Z"));
    }

    #[test]
    fn non_rrule_schedules_are_rejected() {
        let schedule = rrule_schedule(Schedule::every(std::time::Duration::from_secs(30)).unwrap());

        assert!(matches!(
            Recurrence::try_from(&schedule).unwrap_err(),
            RecurrenceError::NotRRule
        ));
    }

    #[test]
    fn malformed_rrule_expression_is_rejected() {
        let schedule = v1::Schedule {
            kind: Some(
                v1::schedule::RRule {
                    dtstart: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&instant(
                        "2026-01-01T00:00:00Z",
                    ))),
                    rrule: "FREQ=INVALID".to_string(),
                    timezone: MessageField::default(),
                    rdate: Vec::new(),
                    exdate: Vec::new(),
                }
                .into(),
            ),
        };

        assert!(matches!(
            plan(&schedule, RRuleCursor::at_or_after(instant("2026-01-01T00:00:00Z"))).unwrap_err(),
            RecurrenceError::Expansion { .. }
        ));
    }

    #[test]
    fn invalid_rrule_timezone_is_rejected() {
        let schedule = v1::Schedule {
            kind: Some(
                v1::schedule::RRule {
                    dtstart: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&instant(
                        "2026-01-01T00:00:00Z",
                    ))),
                    rrule: "FREQ=DAILY;COUNT=1".to_string(),
                    timezone: MessageField::some(trogonai_proto::google::r#type::TimeZone {
                        id: "Not/AZone".to_string(),
                        version: String::new(),
                    }),
                    rdate: Vec::new(),
                    exdate: Vec::new(),
                }
                .into(),
            ),
        };

        assert_eq!(
            Recurrence::try_from(&schedule).unwrap_err(),
            RecurrenceError::Timezone {
                id: "Not/AZone".to_string()
            }
        );
    }
}
