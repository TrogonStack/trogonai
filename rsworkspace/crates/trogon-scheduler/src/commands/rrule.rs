use std::str::FromStr;

use buffa::MessageField;
use chrono::{DateTime, Utc};
use trogonai_proto::convert::{TimestampConversionError, datetime_from_timestamp, timestamp_from_datetime};
use trogonai_proto::google::r#type::TimeZone;
use trogonai_proto::scheduler::schedules::{ScheduleKind, v1};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RRuleCursor {
    AtOrAfter(DateTime<Utc>),
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
pub enum RRuleExpansionError {
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
    #[error("RRULE set could not be expanded: {message}")]
    Expansion { message: String },
}

/// Computes the next recurrence instant for an RRULE [`v1::Schedule`].
///
/// The schedule aggregate owns recurrence calculation; the persisted proto
/// recurrence definition is read directly so the execution layer never has to
/// expand an RRULE itself.
pub(crate) fn next_rrule_occurrence(
    schedule: &v1::Schedule,
    cursor: RRuleCursor,
) -> Result<Option<DateTime<Utc>>, RRuleExpansionError> {
    let Some(ScheduleKind::Rrule(rrule)) = schedule.kind.as_ref() else {
        return Err(RRuleExpansionError::NotRRule);
    };

    let to_utc = |timestamp: &_, field: &'static str| {
        datetime_from_timestamp(timestamp).map_err(|source| RRuleExpansionError::Timestamp { field, source })
    };

    let tz = rrule_timezone(rrule.timezone.as_option())?;
    let dtstart = to_utc(
        rrule
            .dtstart
            .as_option()
            .ok_or(RRuleExpansionError::MissingField { field: "dtstart" })?,
        "dtstart",
    )?
    .with_timezone(&tz);
    let mut set = rrule::RRuleSet::new(dtstart)
        .set_from_string(&format!("RRULE:{}", rrule.rrule))
        .map_err(|source| RRuleExpansionError::Expansion {
            message: source.to_string(),
        })?;

    for date in &rrule.rdate {
        set = set.rdate(to_utc(date, "rdate")?.with_timezone(&tz));
    }
    for date in &rrule.exdate {
        set = set.exdate(to_utc(date, "exdate")?.with_timezone(&tz));
    }

    let instant = cursor.instant();
    let candidates = set.after(instant.with_timezone(&tz)).all(2).dates;
    let occurrence = match cursor {
        RRuleCursor::AtOrAfter(_) => candidates.into_iter().next(),
        RRuleCursor::After(_) => candidates.into_iter().find(|date| date.with_timezone(&Utc) > instant),
    };

    Ok(occurrence.map(|date| date.with_timezone(&Utc)))
}

/// Builds the follow-up aggregate event for a planned recurrence step.
///
/// A `Some(next_occurrence)` plans the next concrete wakeup as
/// [`v1::ScheduleOccurrenceScheduled`]; `None` means the recurrence is exhausted
/// and the schedule is [`v1::ScheduleCompleted`].
pub(crate) fn schedule_or_complete_event(
    schedule_id: &str,
    next_occurrence: Option<DateTime<Utc>>,
    next_sequence: u64,
    last_sequence: u64,
    scheduled_at: DateTime<Utc>,
) -> v1::ScheduleEvent {
    let event = match next_occurrence {
        Some(occurrence_at) => v1::ScheduleOccurrenceScheduled {
            schedule_id: schedule_id.to_string(),
            occurrence_sequence: Some(next_sequence),
            occurrence_at: MessageField::some(timestamp_from_datetime(&occurrence_at)),
            scheduled_at: MessageField::some(timestamp_from_datetime(&scheduled_at)),
        }
        .into(),
        None => v1::ScheduleCompleted {
            schedule_id: schedule_id.to_string(),
            last_occurrence_sequence: Some(last_sequence),
        }
        .into(),
    };

    v1::ScheduleEvent { event: Some(event) }
}

fn rrule_timezone(timezone: Option<&TimeZone>) -> Result<rrule::Tz, RRuleExpansionError> {
    match timezone {
        None => Ok(rrule::Tz::UTC),
        Some(timezone) if timezone.id.is_empty() => Ok(rrule::Tz::UTC),
        Some(timezone) => {
            chrono_tz::Tz::from_str(&timezone.id)
                .map(Into::into)
                .map_err(|_| RRuleExpansionError::Timezone {
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

    #[test]
    fn daily_rrule_returns_the_next_occurrence_at_or_after_the_cursor() {
        let schedule = rrule_schedule(Schedule::rrule("2026-01-01T00:00:00Z", "FREQ=DAILY;COUNT=3", None).unwrap());

        let next = next_rrule_occurrence(&schedule, RRuleCursor::at_or_after(instant("2026-01-01T12:00:00Z"))).unwrap();

        assert_eq!(next, Some(instant("2026-01-02T00:00:00Z")));
    }

    #[test]
    fn strict_after_cursor_does_not_repeat_the_fired_occurrence() {
        let schedule = rrule_schedule(Schedule::rrule("2026-01-01T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap());

        let next = next_rrule_occurrence(&schedule, RRuleCursor::after(instant("2026-01-01T00:00:00Z"))).unwrap();

        assert_eq!(next, Some(instant("2026-01-02T00:00:00Z")));
    }

    #[test]
    fn exhausted_rrule_returns_none() {
        let schedule = rrule_schedule(Schedule::rrule("2026-01-01T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap());

        let next = next_rrule_occurrence(&schedule, RRuleCursor::after(instant("2026-01-02T00:00:00Z"))).unwrap();

        assert_eq!(next, None);
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

        let next = next_rrule_occurrence(&schedule, RRuleCursor::after(instant("2026-01-01T00:00:00Z"))).unwrap();

        assert_eq!(next, Some(instant("2026-01-04T00:00:00Z")));
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

        let next = next_rrule_occurrence(&schedule, RRuleCursor::at_or_after(instant("2026-03-08T12:00:00Z"))).unwrap();

        assert_eq!(next, Some(instant("2026-03-08T13:00:00Z")));
    }

    #[test]
    fn non_rrule_schedules_are_rejected() {
        let schedule = rrule_schedule(Schedule::every(std::time::Duration::from_secs(30)).unwrap());

        let error =
            next_rrule_occurrence(&schedule, RRuleCursor::at_or_after(instant("2026-01-01T00:00:00Z"))).unwrap_err();

        assert!(matches!(error, RRuleExpansionError::NotRRule));
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
            next_rrule_occurrence(&schedule, RRuleCursor::at_or_after(instant("2026-01-01T00:00:00Z"))).unwrap_err(),
            RRuleExpansionError::Expansion { .. }
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
            next_rrule_occurrence(&schedule, RRuleCursor::at_or_after(instant("2026-01-01T00:00:00Z"))).unwrap_err(),
            RRuleExpansionError::Timezone {
                id: "Not/AZone".to_string()
            }
        );
    }
}
