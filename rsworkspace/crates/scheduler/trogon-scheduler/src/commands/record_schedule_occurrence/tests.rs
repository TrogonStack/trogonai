use buffa::{EnumValue, MessageField};
use chrono::TimeZone;
use trogon_decider::testing::TestCase;

use super::*;
use crate::commands::domain::{Schedule as DomainSchedule, ScheduleEventSchedule};

fn schedule_id(id: &str) -> ScheduleId {
    ScheduleId::parse(id).unwrap()
}

fn occurrence_at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 6, 4, 0, 0, 0).unwrap()
}

fn recorded_at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 6, 4, 0, 0, 7).unwrap()
}

fn rrule_schedule(count: u32) -> v1::Schedule {
    v1::Schedule::try_from(&ScheduleEventSchedule::from(
        &DomainSchedule::rrule("2026-06-03T00:00:00Z", format!("FREQ=DAILY;COUNT={count}"), None).unwrap(),
    ))
    .unwrap()
}

fn timestamp(at: DateTime<Utc>) -> MessageField<buffa_types::google::protobuf::Timestamp> {
    MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&at))
}

fn record(id: &str) -> RecordScheduleOccurrence {
    RecordScheduleOccurrence::new(schedule_id(id), occurrence_at(), recorded_at())
}

#[test]
fn decider_identity_delegates_to_schedule_state() {
    let command = record("recurring");
    assert_eq!(command.stream_id(), "recurring");
    assert_eq!(
        RecordScheduleOccurrence::initial_state(),
        super::super::state::initial_state()
    );
}

fn recorded(id: &str, sequence: u64, at: DateTime<Utc>) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleOccurrenceRecorded {
                schedule_id: id.to_string(),
                occurrence_sequence: Some(sequence),
                occurrence_at: timestamp(at),
                recorded_at: timestamp(recorded_at()),
            }
            .into(),
        ),
    }
}

fn scheduled(id: &str, sequence: u64, at: DateTime<Utc>) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleOccurrenceScheduled {
                schedule_id: id.to_string(),
                occurrence_sequence: Some(sequence),
                occurrence_at: timestamp(at),
                scheduled_at: timestamp(recorded_at()),
            }
            .into(),
        ),
    }
}

fn completed(id: &str, last_sequence: u64) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleCompleted {
                schedule_id: id.to_string(),
                last_occurrence_sequence: Some(last_sequence),
            }
            .into(),
        ),
    }
}

fn created(id: &str, enabled: bool, schedule: v1::Schedule) -> v1::ScheduleEvent {
    let kind = if enabled {
        v1::schedule_status::Scheduled {}.into()
    } else {
        v1::schedule_status::Paused {}.into()
    };
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleCreated {
                schedule_id: id.to_string(),
                status: MessageField::some(v1::ScheduleStatus { kind: Some(kind) }),
                schedule: MessageField::some(schedule),
                delivery: MessageField::default(),
                message: MessageField::default(),
            }
            .into(),
        ),
    }
}

fn removed(id: &str) -> v1::ScheduleEvent {
    v1::ScheduleEvent {
        event: Some(
            v1::ScheduleRemoved {
                schedule_id: id.to_string(),
            }
            .into(),
        ),
    }
}

#[test]
fn records_occurrence_and_schedules_the_next_one() {
    let id = "recurring";

    TestCase::<RecordScheduleOccurrence>::new()
        .given([created(id, true, rrule_schedule(3)), scheduled(id, 1, occurrence_at())])
        .when(record(id))
        .then([
            recorded(id, 1, occurrence_at()),
            scheduled(id, 2, Utc.with_ymd_and_hms(2026, 6, 5, 0, 0, 0).unwrap()),
        ]);
}

#[test]
fn records_final_occurrence_and_completes() {
    let id = "recurring";

    TestCase::<RecordScheduleOccurrence>::new()
        .given([
            created(id, true, rrule_schedule(2)),
            recorded(id, 1, Utc.with_ymd_and_hms(2026, 6, 3, 0, 0, 0).unwrap()),
            scheduled(id, 2, occurrence_at()),
        ])
        .when(record(id))
        .then([recorded(id, 2, occurrence_at()), completed(id, 2)]);
}

#[test]
fn records_paused_pending_occurrence_without_scheduling_follow_up() {
    let id = "recurring";

    TestCase::<RecordScheduleOccurrence>::new()
        .given([created(id, false, rrule_schedule(3)), scheduled(id, 1, occurrence_at())])
        .when(record(id))
        .then([recorded(id, 1, occurrence_at())]);
}

#[test]
fn records_final_occurrence_while_paused_completes() {
    let id = "recurring";

    TestCase::<RecordScheduleOccurrence>::new()
        .given([
            created(id, false, rrule_schedule(2)),
            recorded(id, 1, Utc.with_ymd_and_hms(2026, 6, 3, 0, 0, 0).unwrap()),
            scheduled(id, 2, occurrence_at()),
        ])
        .when(record(id))
        .then([recorded(id, 2, occurrence_at()), completed(id, 2)]);
}

#[test]
fn rejects_missing_and_deleted_schedules() {
    let id = "recurring";

    TestCase::<RecordScheduleOccurrence>::new()
        .given_no_history()
        .when(record(id))
        .then_error(RecordScheduleOccurrenceError::ScheduleNotFound { id: schedule_id(id) });

    TestCase::<RecordScheduleOccurrence>::new()
        .given([created(id, true, rrule_schedule(2)), removed(id)])
        .when(record(id))
        .then_error(RecordScheduleOccurrenceError::ScheduleDeleted { id: schedule_id(id) });
}

#[test]
fn rejects_duplicate_or_stale_occurrences() {
    let id = "recurring";
    let last_recorded_at = occurrence_at();

    TestCase::<RecordScheduleOccurrence>::new()
        .given([
            created(id, true, rrule_schedule(2)),
            recorded(id, 1, last_recorded_at),
            scheduled(id, 2, occurrence_at()),
        ])
        .when(record(id))
        .then_error(RecordScheduleOccurrenceError::OccurrenceAlreadyRecorded {
            id: schedule_id(id),
            occurrence_at: occurrence_at(),
            last_recorded_at,
        });
}

#[test]
fn rejects_occurrences_that_were_not_planned() {
    let id = "recurring";

    TestCase::<RecordScheduleOccurrence>::new()
        .given([created(id, true, rrule_schedule(2))])
        .when(record(id))
        .then_error(RecordScheduleOccurrenceError::OccurrenceNotPending {
            id: schedule_id(id),
            occurrence_at: occurrence_at(),
            pending_occurrence_at: None,
        });

    TestCase::<RecordScheduleOccurrence>::new()
        .given([created(id, false, rrule_schedule(2))])
        .when(record(id))
        .then_error(RecordScheduleOccurrenceError::OccurrenceNotPending {
            id: schedule_id(id),
            occurrence_at: occurrence_at(),
            pending_occurrence_at: None,
        });

    let other_pending = Utc.with_ymd_and_hms(2026, 6, 5, 0, 0, 0).unwrap();
    TestCase::<RecordScheduleOccurrence>::new()
        .given([created(id, true, rrule_schedule(3)), scheduled(id, 1, other_pending)])
        .when(record(id))
        .then_error(RecordScheduleOccurrenceError::OccurrenceNotPending {
            id: schedule_id(id),
            occurrence_at: occurrence_at(),
            pending_occurrence_at: Some(other_pending),
        });
}

#[test]
fn rejects_occurrence_sequence_overflow() {
    let id = "recurring";

    TestCase::<RecordScheduleOccurrence>::new()
        .given([
            created(id, true, rrule_schedule(2)),
            recorded(id, u64::MAX, Utc.with_ymd_and_hms(2026, 6, 3, 0, 0, 0).unwrap()),
            scheduled(id, 2, occurrence_at()),
        ])
        .when(record(id))
        .then_error(RecordScheduleOccurrenceError::OccurrenceSequence {
            source: ScheduleOccurrenceSequenceError::Overflow,
        });
}

#[test]
fn rejects_malformed_state_values() {
    let id = "recurring";

    TestCase::<RecordScheduleOccurrence>::new()
        .given_state(state_v1::State {
            completed: None,
            state: None,
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        })
        .when(record(id))
        .then_error(RecordScheduleOccurrenceError::MissingStateValue);

    TestCase::<RecordScheduleOccurrence>::new()
        .given_state(state_v1::State {
            completed: None,
            state: Some(EnumValue::from(123)),
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        })
        .when(record(id))
        .then_error(RecordScheduleOccurrenceError::UnknownStateValue { value: 123 });

    TestCase::<RecordScheduleOccurrence>::new()
        .given_state(state_v1::State {
            completed: None,
            state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_UNSPECIFIED)),
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence: None,
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
        })
        .when(record(id))
        .then_error(RecordScheduleOccurrenceError::UnknownStateValue { value: 0 });
}
