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
