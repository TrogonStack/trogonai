use buffa::{EnumValue, MessageField};
    use chrono::TimeZone;
    use trogon_decider::testing::TestCase;

    use super::*;
    use crate::commands::domain::{Schedule as DomainSchedule, ScheduleEventSchedule};

    fn schedule_id(id: &str) -> ScheduleId {
        ScheduleId::parse(id).unwrap()
    }

    fn rrule_schedule(count: u32) -> v1::Schedule {
        v1::Schedule::try_from(&ScheduleEventSchedule::from(
            &DomainSchedule::rrule("2026-06-03T00:00:00Z", format!("FREQ=DAILY;COUNT={count}"), None).unwrap(),
        ))
        .unwrap()
    }

    fn enabled_state(
        last_occurrence_sequence: Option<u64>,
        pending_occurrence_at: Option<DateTime<Utc>>,
        schedule: MessageField<v1::Schedule>,
    ) -> state_v1::State {
        state_v1::State {
            completed: None,
            state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)),
            last_occurrence_at: MessageField::default(),
            last_occurrence_sequence,
            schedule,
            pending_occurrence_at: pending_occurrence_at
                .map(|at| MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&at)))
                .unwrap_or_default(),
        }
    }

    fn command(id: &str, now: DateTime<Utc>) -> ScheduleNextOccurrence {
        ScheduleNextOccurrence::new(schedule_id(id), now)
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

    fn recorded(id: &str, sequence: u64, at: DateTime<Utc>) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleOccurrenceRecorded {
                    schedule_id: id.to_string(),
                    occurrence_sequence: Some(sequence),
                    occurrence_at: MessageField::some(timestamp_from_datetime(&at)),
                    recorded_at: MessageField::some(timestamp_from_datetime(&at)),
                }
                .into(),
            ),
        }
    }

    fn occurrence_scheduled(
        id: &str,
        sequence: u64,
        occurrence_at: DateTime<Utc>,
        scheduled_at: DateTime<Utc>,
    ) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleOccurrenceScheduled {
                    schedule_id: id.to_string(),
                    occurrence_sequence: Some(sequence),
                    occurrence_at: MessageField::some(timestamp_from_datetime(&occurrence_at)),
                    scheduled_at: MessageField::some(timestamp_from_datetime(&scheduled_at)),
                }
                .into(),
            ),
        }
    }

    fn occurrence_completed(id: &str, last_sequence: u64) -> v1::ScheduleEvent {
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

    #[test]
    fn decider_identity_delegates_to_schedule_state() {
        let command = command("recurring", Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap());
        assert_eq!(command.stream_id(), "recurring");
        assert_eq!(
            ScheduleNextOccurrence::initial_state(),
            super::super::state::initial_state()
        );
    }

    #[test]
    fn arms_the_first_occurrence_for_a_created_schedule() {
        let id = "recurring";
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();

        TestCase::<ScheduleNextOccurrence>::new()
            .given([created(id, true, rrule_schedule(3))])
            .when(command(id, now))
            .then([occurrence_scheduled(
                id,
                1,
                Utc.with_ymd_and_hms(2026, 6, 3, 0, 0, 0).unwrap(),
                now,
            )]);
    }

    #[test]
    fn arms_after_the_last_recent_recorded_occurrence() {
        let id = "recurring";
        let last = Utc.with_ymd_and_hms(2026, 6, 3, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 6, 4, 0, 0, 0).unwrap();

        TestCase::<ScheduleNextOccurrence>::new()
            .given([created(id, true, rrule_schedule(4)), recorded(id, 1, last)])
            .when(command(id, now))
            .then([occurrence_scheduled(
                id,
                2,
                Utc.with_ymd_and_hms(2026, 6, 4, 0, 0, 0).unwrap(),
                now,
            )]);
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn arms_after_old_recorded_occurrence_without_grace_skip() {
        let id = "recurring";
        let last = Utc.with_ymd_and_hms(2026, 6, 3, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 6, 10, 0, 0, 0).unwrap();

        TestCase::<ScheduleNextOccurrence>::new()
            .given([created(id, true, rrule_schedule(10)), recorded(id, 1, last)])
            .when(command(id, now))
            .then([occurrence_scheduled(
                id,
                2,
                Utc.with_ymd_and_hms(2026, 6, 4, 0, 0, 0).unwrap(),
                now,
            )]);
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn arms_the_occurrence_strictly_after_the_last_recorded_one() {
        let id = "recurring";
        let last = Utc.with_ymd_and_hms(2026, 6, 4, 0, 0, 0).unwrap();
        // Resume shortly after the last recorded occurrence: the cursor must skip
        // it and arm the next one, not re-arm the one already recorded.
        let now = Utc.with_ymd_and_hms(2026, 6, 4, 0, 1, 0).unwrap();

        TestCase::<ScheduleNextOccurrence>::new()
            .given([created(id, true, rrule_schedule(3)), recorded(id, 2, last)])
            .when(command(id, now))
            .then([occurrence_scheduled(
                id,
                3,
                Utc.with_ymd_and_hms(2026, 6, 5, 0, 0, 0).unwrap(),
                now,
            )]);
    }

    #[test]
    fn completes_when_no_future_occurrence_remains() {
        let id = "recurring";
        let now = Utc.with_ymd_and_hms(2026, 6, 10, 0, 0, 0).unwrap();

        TestCase::<ScheduleNextOccurrence>::new()
            .given([created(id, true, rrule_schedule(2))])
            .when(command(id, now))
            .then([occurrence_completed(id, 0)]);
    }

    #[test]
    fn rejects_occurrence_sequence_overflow() {
        let id = "recurring";
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();

        TestCase::<ScheduleNextOccurrence>::new()
            .given_state(enabled_state(
                Some(u64::MAX),
                None,
                MessageField::some(rrule_schedule(3)),
            ))
            .when(command(id, now))
            .then_error(ScheduleNextOccurrenceError::OccurrenceSequence {
                source: ScheduleOccurrenceSequenceError::Overflow,
            });
    }

    #[test]
    fn rejects_when_already_armed() {
        let id = "recurring";
        let pending = Utc.with_ymd_and_hms(2026, 6, 3, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();

        TestCase::<ScheduleNextOccurrence>::new()
            .given([
                created(id, true, rrule_schedule(3)),
                occurrence_scheduled(id, 1, pending, pending),
            ])
            .when(command(id, now))
            .then_error(ScheduleNextOccurrenceError::AlreadyArmed { id: schedule_id(id) });
    }

    #[test]
    fn rejects_when_already_completed() {
        let id = "recurring";
        let now = Utc.with_ymd_and_hms(2026, 6, 10, 0, 0, 0).unwrap();

        TestCase::<ScheduleNextOccurrence>::new()
            .given([created(id, true, rrule_schedule(2)), occurrence_completed(id, 2)])
            .when(command(id, now))
            .then_error(ScheduleNextOccurrenceError::AlreadyCompleted { id: schedule_id(id) });
    }

    #[test]
    fn rejects_paused_deleted_and_missing_schedules() {
        let id = "recurring";
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();

        TestCase::<ScheduleNextOccurrence>::new()
            .given_no_history()
            .when(command(id, now))
            .then_error(ScheduleNextOccurrenceError::ScheduleNotFound { id: schedule_id(id) });

        TestCase::<ScheduleNextOccurrence>::new()
            .given([created(id, false, rrule_schedule(3))])
            .when(command(id, now))
            .then_error(ScheduleNextOccurrenceError::SchedulePaused { id: schedule_id(id) });

        TestCase::<ScheduleNextOccurrence>::new()
            .given([created(id, true, rrule_schedule(3)), removed(id)])
            .when(command(id, now))
            .then_error(ScheduleNextOccurrenceError::ScheduleDeleted { id: schedule_id(id) });
    }

    #[test]
    fn rejects_when_schedule_definition_is_missing() {
        let id = "recurring";
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();

        TestCase::<ScheduleNextOccurrence>::new()
            .given_state(enabled_state(None, None, MessageField::default()))
            .when(command(id, now))
            .then_error(ScheduleNextOccurrenceError::MissingSchedule { id: schedule_id(id) });
    }

    #[test]
    fn rejects_invalid_last_recorded_timestamp() {
        let id = "recurring";
        let invalid = buffa_types::google::protobuf::Timestamp {
            seconds: i64::MAX,
            nanos: 0,
            ..Default::default()
        };
        let source = trogonai_proto::convert::datetime_from_timestamp(&invalid).unwrap_err();
        let state = state_v1::State {
            completed: None,
            state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)),
            last_occurrence_at: MessageField::some(invalid),
            last_occurrence_sequence: Some(1),
            schedule: MessageField::some(rrule_schedule(3)),
            pending_occurrence_at: MessageField::default(),
        };
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();

        TestCase::<ScheduleNextOccurrence>::new()
            .given_state(state)
            .when(command(id, now))
            .then_error(ScheduleNextOccurrenceError::LastRecordedAt { source });
    }

    #[test]
    fn rejects_malformed_state_values() {
        let id = "recurring";
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();

        TestCase::<ScheduleNextOccurrence>::new()
            .given_state(state_v1::State {
                completed: None,
                state: None,
                last_occurrence_at: MessageField::default(),
                last_occurrence_sequence: None,
                schedule: MessageField::default(),
                pending_occurrence_at: MessageField::default(),
            })
            .when(command(id, now))
            .then_error(ScheduleNextOccurrenceError::MissingStateValue);

        TestCase::<ScheduleNextOccurrence>::new()
            .given_state(state_v1::State {
                completed: None,
                state: Some(EnumValue::from(123)),
                last_occurrence_at: MessageField::default(),
                last_occurrence_sequence: None,
                schedule: MessageField::default(),
                pending_occurrence_at: MessageField::default(),
            })
            .when(command(id, now))
            .then_error(ScheduleNextOccurrenceError::UnknownStateValue { value: 123 });

        TestCase::<ScheduleNextOccurrence>::new()
            .given_state(state_v1::State {
                completed: None,
                state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_UNSPECIFIED)),
                last_occurrence_at: MessageField::default(),
                last_occurrence_sequence: None,
                schedule: MessageField::default(),
                pending_occurrence_at: MessageField::default(),
            })
            .when(command(id, now))
            .then_error(ScheduleNextOccurrenceError::UnknownStateValue { value: 0 });
    }
