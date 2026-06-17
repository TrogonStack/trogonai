use buffa::MessageField;
use chrono::{DateTime, Utc};
use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot};
use trogonai_proto::convert::{TimestampConversionError, timestamp_from_datetime};
use trogonai_proto::scheduler::schedules::{state_v1, v1};

use super::domain::{
    RRuleCursor, Recurrence, RecurrenceError, RecurrenceStep, ScheduleId, ScheduleOccurrenceSequence,
    ScheduleOccurrenceSequenceError,
};

/// Occurrences whose due instant is at most this far in the past are still armed,
/// absorbing scheduling/processing latency around recently due occurrences.
const PAST_OCCURRENCE_GRACE: chrono::Duration = chrono::Duration::minutes(5);

/// Arms the next recurrence occurrence for an enabled RRULE schedule.
///
/// Issued by the execution processor when it observes a schedule becoming
/// schedulable (created or resumed). The aggregate calculates the next concrete
/// occurrence and records it as a durable plan, leaving the processor to publish
/// the wakeup without any recurrence math of its own.
#[derive(Debug, Clone)]
pub struct ScheduleNextOccurrence {
    pub id: ScheduleId,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleNextOccurrenceError {
    #[error("schedule '{id}' does not exist")]
    ScheduleNotFound { id: ScheduleId },
    #[error("schedule '{id}' was deleted")]
    ScheduleDeleted { id: ScheduleId },
    #[error("schedule '{id}' is paused")]
    SchedulePaused { id: ScheduleId },
    #[error("schedule '{id}' already has a pending occurrence")]
    AlreadyArmed { id: ScheduleId },
    #[error("schedule '{id}' has already completed its recurrence")]
    AlreadyCompleted { id: ScheduleId },
    #[error("schedule '{id}' is missing its recurrence definition")]
    MissingSchedule { id: ScheduleId },
    #[error("last recorded occurrence timestamp is invalid: {source}")]
    LastRecordedAt {
        #[source]
        source: TimestampConversionError,
    },
    #[error("next recurrence occurrence could not be calculated: {source}")]
    NextOccurrence {
        #[source]
        source: RecurrenceError,
    },
    #[error("schedule occurrence sequence could not advance: {source}")]
    OccurrenceSequence {
        #[source]
        source: ScheduleOccurrenceSequenceError,
    },
    #[error("state value is missing")]
    MissingStateValue,
    #[error("unknown state value: {value}")]
    UnknownStateValue { value: i32 },
}

impl ScheduleNextOccurrence {
    pub fn new(id: ScheduleId, now: DateTime<Utc>) -> Self {
        Self { id, now }
    }
}

impl Decider for ScheduleNextOccurrence {
    type StreamId = str;
    type State = state_v1::State;
    type Event = v1::ScheduleEvent;
    type DecideError = ScheduleNextOccurrenceError;
    type EvolveError = super::EvolveError;

    fn stream_id(&self) -> &Self::StreamId {
        self.id.as_str()
    }

    fn initial_state() -> Self::State {
        super::state::initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        super::state::evolve(state, event)
    }

    fn decide(state: &state_v1::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let Some(value) = state.state.as_ref() else {
            return Err(ScheduleNextOccurrenceError::MissingStateValue);
        };
        let Some(current_state) = value.as_known() else {
            return Err(ScheduleNextOccurrenceError::UnknownStateValue { value: value.to_i32() });
        };
        match current_state {
            state_v1::StateValue::STATE_VALUE_MISSING => {
                Err(ScheduleNextOccurrenceError::ScheduleNotFound { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_DELETED => {
                Err(ScheduleNextOccurrenceError::ScheduleDeleted { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED => {
                Err(ScheduleNextOccurrenceError::SchedulePaused { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED => {
                if state.completed == Some(true) {
                    return Err(ScheduleNextOccurrenceError::AlreadyCompleted { id: command.id.clone() });
                }
                if state.pending_occurrence_at.is_set() {
                    return Err(ScheduleNextOccurrenceError::AlreadyArmed { id: command.id.clone() });
                }

                let schedule = state
                    .schedule
                    .as_option()
                    .ok_or_else(|| ScheduleNextOccurrenceError::MissingSchedule { id: command.id.clone() })?;

                let last_sequence = state.last_occurrence_sequence.unwrap_or(0);
                let last_occurrence_at = match state.last_occurrence_at.as_option() {
                    Some(last) => Some(
                        trogonai_proto::convert::datetime_from_timestamp(last)
                            .map_err(|source| ScheduleNextOccurrenceError::LastRecordedAt { source })?,
                    ),
                    None => None,
                };

                let floor = command.now - PAST_OCCURRENCE_GRACE;
                let cursor = match last_occurrence_at {
                    Some(last) => RRuleCursor::after(last),
                    None => RRuleCursor::at_or_after(floor),
                };

                let recurrence = Recurrence::try_from(schedule)
                    .map_err(|source| ScheduleNextOccurrenceError::NextOccurrence { source })?;
                let step = recurrence
                    .plan_next(cursor)
                    .map_err(|source| ScheduleNextOccurrenceError::NextOccurrence { source })?;

                let event = recurrence_event(command.id.as_str(), step, last_sequence, command.now)?;

                Ok(Decision::event(event))
            }
            state_v1::StateValue::STATE_VALUE_UNSPECIFIED => {
                Err(ScheduleNextOccurrenceError::UnknownStateValue { value: 0 })
            }
        }
    }
}

/// Translates the aggregate's recurrence decision into the schedule event the
/// command raises, advancing the gapless occurrence sequence when one is armed.
fn recurrence_event(
    schedule_id: &str,
    step: RecurrenceStep,
    last_sequence: u64,
    scheduled_at: DateTime<Utc>,
) -> Result<v1::ScheduleEvent, ScheduleNextOccurrenceError> {
    let event = match step {
        RecurrenceStep::Occurrence { at } => {
            let sequence = ScheduleOccurrenceSequence::next_after(last_sequence)
                .map_err(|source| ScheduleNextOccurrenceError::OccurrenceSequence { source })?;
            v1::ScheduleOccurrenceScheduled {
                schedule_id: schedule_id.to_string(),
                occurrence_sequence: Some(sequence.as_u64()),
                occurrence_at: MessageField::some(timestamp_from_datetime(&at)),
                scheduled_at: MessageField::some(timestamp_from_datetime(&scheduled_at)),
            }
            .into()
        }
        RecurrenceStep::Exhausted => v1::ScheduleCompleted {
            schedule_id: schedule_id.to_string(),
            last_occurrence_sequence: Some(last_sequence),
        }
        .into(),
    };

    Ok(v1::ScheduleEvent { event: Some(event) })
}

impl CommandSnapshotPolicy for ScheduleNextOccurrence {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::COMMAND_SNAPSHOT_POLICY;
}

#[cfg(test)]
mod tests {
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
        let state = state_v1::State {
            completed: None,
            state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)),
            last_occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&last)),
            last_occurrence_sequence: Some(1),
            schedule: MessageField::some(rrule_schedule(10)),
            pending_occurrence_at: MessageField::default(),
        };

        let decision = ScheduleNextOccurrence::decide(&state, &command(id, now)).unwrap();
        let Decision::Events(events) = decision else {
            panic!("expected an arming decision");
        };

        assert_eq!(
            events.as_slice(),
            &[v1::ScheduleEvent {
                event: Some(
                    v1::ScheduleOccurrenceScheduled {
                        schedule_id: id.to_string(),
                        occurrence_sequence: Some(2),
                        occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(
                            &Utc.with_ymd_and_hms(2026, 6, 4, 0, 0, 0).unwrap()
                        )),
                        scheduled_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&now)),
                    }
                    .into(),
                ),
            }]
        );
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn arms_the_occurrence_strictly_after_the_last_recorded_one() {
        let id = "recurring";
        let last = Utc.with_ymd_and_hms(2026, 6, 4, 0, 0, 0).unwrap();
        let state = state_v1::State {
            completed: None,
            state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)),
            last_occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&last)),
            last_occurrence_sequence: Some(2),
            schedule: MessageField::some(rrule_schedule(3)),
            pending_occurrence_at: MessageField::default(),
        };
        // Resume shortly after the last recorded occurrence: the cursor must skip
        // it and arm the next one, not re-arm the one already recorded.
        let now = Utc.with_ymd_and_hms(2026, 6, 4, 0, 1, 0).unwrap();

        let decision = ScheduleNextOccurrence::decide(&state, &command(id, now)).unwrap();
        let Decision::Events(events) = decision else {
            panic!("expected an arming decision");
        };

        assert_eq!(
            events.as_slice(),
            &[v1::ScheduleEvent {
                event: Some(
                    v1::ScheduleOccurrenceScheduled {
                        schedule_id: id.to_string(),
                        occurrence_sequence: Some(3),
                        occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(
                            &Utc.with_ymd_and_hms(2026, 6, 5, 0, 0, 0).unwrap()
                        )),
                        scheduled_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&now)),
                    }
                    .into(),
                ),
            }]
        );
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
}
