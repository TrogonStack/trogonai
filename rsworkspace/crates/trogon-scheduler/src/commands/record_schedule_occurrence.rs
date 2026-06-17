use chrono::{DateTime, Utc};
use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, Events, FrequencySnapshot};
use trogonai_proto::convert::TimestampConversionError;
use trogonai_proto::scheduler::schedules::{state_v1, v1};

use super::domain::{
    RRuleCursor, Recurrence, RecurrenceError, RecurrenceStep, ScheduleId, ScheduleOccurrenceSequence,
    ScheduleOccurrenceSequenceError,
};

#[derive(Debug, Clone)]
pub struct RecordScheduleOccurrence {
    pub id: ScheduleId,
    pub occurrence_at: DateTime<Utc>,
    /// Wall-clock instant the occurrence is being recorded; audit only.
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecordScheduleOccurrenceError {
    #[error("schedule '{id}' does not exist")]
    ScheduleNotFound { id: ScheduleId },
    #[error("schedule '{id}' was deleted")]
    ScheduleDeleted { id: ScheduleId },
    #[error("schedule '{id}' already recorded occurrence at {last_recorded_at}")]
    OccurrenceAlreadyRecorded {
        id: ScheduleId,
        occurrence_at: DateTime<Utc>,
        last_recorded_at: DateTime<Utc>,
    },
    #[error("schedule '{id}' did not plan an occurrence at {occurrence_at}")]
    OccurrenceNotPending {
        id: ScheduleId,
        occurrence_at: DateTime<Utc>,
        pending_occurrence_at: Option<DateTime<Utc>>,
    },
    #[error("schedule '{id}' is missing its recurrence definition")]
    MissingSchedule { id: ScheduleId },
    #[error("last recorded occurrence timestamp is invalid: {source}")]
    LastRecordedAt {
        #[source]
        source: TimestampConversionError,
    },
    #[error("pending occurrence timestamp is invalid: {source}")]
    PendingOccurrenceAt {
        #[source]
        source: TimestampConversionError,
    },
    #[error("schedule occurrence sequence could not advance: {source}")]
    OccurrenceSequence {
        #[source]
        source: ScheduleOccurrenceSequenceError,
    },
    #[error("next recurrence occurrence could not be calculated: {source}")]
    NextOccurrence {
        #[source]
        source: RecurrenceError,
    },
    #[error("state value is missing")]
    MissingStateValue,
    #[error("unknown state value: {value}")]
    UnknownStateValue { value: i32 },
}

impl RecordScheduleOccurrence {
    pub fn new(id: ScheduleId, occurrence_at: DateTime<Utc>, recorded_at: DateTime<Utc>) -> Self {
        Self {
            id,
            occurrence_at,
            recorded_at,
        }
    }
}

impl Decider for RecordScheduleOccurrence {
    type StreamId = str;
    type State = state_v1::State;
    type Event = v1::ScheduleEvent;
    type DecideError = RecordScheduleOccurrenceError;
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
            return Err(RecordScheduleOccurrenceError::MissingStateValue);
        };
        let Some(current_state) = value.as_known() else {
            return Err(RecordScheduleOccurrenceError::UnknownStateValue { value: value.to_i32() });
        };
        match current_state {
            state_v1::StateValue::STATE_VALUE_MISSING => {
                Err(RecordScheduleOccurrenceError::ScheduleNotFound { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_DELETED => {
                Err(RecordScheduleOccurrenceError::ScheduleDeleted { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED | state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED => {
                if let Some(last_recorded_at) = state.last_occurrence_at.as_option() {
                    let last_recorded_at = trogonai_proto::convert::datetime_from_timestamp(last_recorded_at)
                        .map_err(|source| RecordScheduleOccurrenceError::LastRecordedAt { source })?;
                    if last_recorded_at >= command.occurrence_at {
                        return Err(RecordScheduleOccurrenceError::OccurrenceAlreadyRecorded {
                            id: command.id.clone(),
                            occurrence_at: command.occurrence_at,
                            last_recorded_at,
                        });
                    }
                }

                // A matching pending occurrence proves this wakeup was armed before
                // pause/remove races. Paused aggregates record that progress without
                // planning another wakeup.
                let pending_occurrence_at = match state.pending_occurrence_at.as_option() {
                    Some(pending) => Some(
                        trogonai_proto::convert::datetime_from_timestamp(pending)
                            .map_err(|source| RecordScheduleOccurrenceError::PendingOccurrenceAt { source })?,
                    ),
                    None => None,
                };
                if pending_occurrence_at != Some(command.occurrence_at) {
                    return Err(RecordScheduleOccurrenceError::OccurrenceNotPending {
                        id: command.id.clone(),
                        occurrence_at: command.occurrence_at,
                        pending_occurrence_at,
                    });
                }

                let occurrence_sequence =
                    ScheduleOccurrenceSequence::next_after(state.last_occurrence_sequence.unwrap_or(0))
                        .map_err(|source| RecordScheduleOccurrenceError::OccurrenceSequence { source })?;

                let recorded = v1::ScheduleEvent {
                    event: Some(
                        v1::ScheduleOccurrenceRecorded {
                            schedule_id: command.id.as_str().to_string(),
                            occurrence_sequence: Some(occurrence_sequence.as_u64()),
                            occurrence_at: buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(
                                &command.occurrence_at,
                            )),
                            recorded_at: buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(
                                &command.recorded_at,
                            )),
                        }
                        .into(),
                    ),
                };
                let schedule = state
                    .schedule
                    .as_option()
                    .ok_or_else(|| RecordScheduleOccurrenceError::MissingSchedule { id: command.id.clone() })?;
                let recurrence = Recurrence::try_from(schedule)
                    .map_err(|source| RecordScheduleOccurrenceError::NextOccurrence { source })?;
                let step = recurrence
                    .plan_next(RRuleCursor::after(command.occurrence_at))
                    .map_err(|source| RecordScheduleOccurrenceError::NextOccurrence { source })?;

                // A paused schedule never arms the next wakeup, but an exhausted
                // recurrence is still finished: it must still complete, just without
                // planning a follow-up occurrence.
                if current_state == state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED
                    && matches!(step, RecurrenceStep::Occurrence { .. })
                {
                    return Ok(Decision::event(recorded));
                }

                let follow_up = recurrence_event(
                    command.id.as_str(),
                    step,
                    occurrence_sequence.as_u64(),
                    command.recorded_at,
                )?;

                Ok(Decision::events(Events::from_first(recorded, vec![follow_up])))
            }
            state_v1::StateValue::STATE_VALUE_UNSPECIFIED => {
                Err(RecordScheduleOccurrenceError::UnknownStateValue { value: 0 })
            }
        }
    }
}

/// Translates the aggregate's recurrence decision into the follow-up schedule
/// event the command raises, advancing the gapless occurrence sequence when a
/// further occurrence is armed.
fn recurrence_event(
    schedule_id: &str,
    step: RecurrenceStep,
    last_sequence: u64,
    scheduled_at: DateTime<Utc>,
) -> Result<v1::ScheduleEvent, RecordScheduleOccurrenceError> {
    let event = match step {
        RecurrenceStep::Occurrence { at } => {
            let sequence = ScheduleOccurrenceSequence::next_after(last_sequence)
                .map_err(|source| RecordScheduleOccurrenceError::OccurrenceSequence { source })?;
            v1::ScheduleOccurrenceScheduled {
                schedule_id: schedule_id.to_string(),
                occurrence_sequence: Some(sequence.as_u64()),
                occurrence_at: buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&at)),
                scheduled_at: buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(
                    &scheduled_at,
                )),
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

impl CommandSnapshotPolicy for RecordScheduleOccurrence {
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

    fn present_state(
        value: state_v1::StateValue,
        last_occurrence_at: Option<DateTime<Utc>>,
        last_occurrence_sequence: Option<u64>,
        pending_occurrence_at: Option<DateTime<Utc>>,
        schedule: MessageField<v1::Schedule>,
    ) -> state_v1::State {
        state_v1::State {
            completed: None,
            state: Some(EnumValue::from(value)),
            last_occurrence_at: last_occurrence_at.map(timestamp).unwrap_or_default(),
            last_occurrence_sequence,
            schedule,
            pending_occurrence_at: pending_occurrence_at.map(timestamp).unwrap_or_default(),
        }
    }

    fn enabled_state(
        last_occurrence_at: Option<DateTime<Utc>>,
        last_occurrence_sequence: Option<u64>,
        pending_occurrence_at: Option<DateTime<Utc>>,
        schedule: MessageField<v1::Schedule>,
    ) -> state_v1::State {
        present_state(
            state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED,
            last_occurrence_at,
            last_occurrence_sequence,
            pending_occurrence_at,
            schedule,
        )
    }

    fn record(id: &str) -> RecordScheduleOccurrence {
        RecordScheduleOccurrence::new(schedule_id(id), occurrence_at(), recorded_at())
    }

    #[test]
    #[allow(clippy::disallowed_methods, reason = "delegation test: asserts Decider::evolve forwards to the schedule state module")]
    fn decider_identity_delegates_to_schedule_state() {
        let command = record("recurring");
        assert_eq!(command.stream_id(), "recurring");
        assert_eq!(
            RecordScheduleOccurrence::initial_state(),
            super::super::state::initial_state()
        );

        let event = recorded("recurring", 1, occurrence_at());
        let evolved = RecordScheduleOccurrence::evolve(
            enabled_state(None, None, Some(occurrence_at()), MessageField::default()),
            &event,
        )
        .unwrap();
        assert_eq!(evolved.last_occurrence_sequence, Some(1));
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
    #[allow(clippy::disallowed_methods, reason = "exercises decide's guard against corrupt persisted state values that no event replay can produce")]
    fn rejects_malformed_state_values() {
        let id = "recurring";

        assert_eq!(
            RecordScheduleOccurrence::decide(
                &state_v1::State {
                    completed: None,
                    state: None,
                    last_occurrence_at: MessageField::default(),
                    last_occurrence_sequence: None,
                    schedule: MessageField::default(),
                    pending_occurrence_at: MessageField::default(),
                },
                &record(id)
            )
            .unwrap_err(),
            RecordScheduleOccurrenceError::MissingStateValue
        );
        assert_eq!(
            RecordScheduleOccurrence::decide(
                &state_v1::State {
                    completed: None,
                    state: Some(EnumValue::from(123)),
                    last_occurrence_at: MessageField::default(),
                    last_occurrence_sequence: None,
                    schedule: MessageField::default(),
                    pending_occurrence_at: MessageField::default(),
                },
                &record(id)
            )
            .unwrap_err(),
            RecordScheduleOccurrenceError::UnknownStateValue { value: 123 }
        );
        assert_eq!(
            RecordScheduleOccurrence::decide(
                &state_v1::State {
                    completed: None,
                    state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_UNSPECIFIED)),
                    last_occurrence_at: MessageField::default(),
                    last_occurrence_sequence: None,
                    schedule: MessageField::default(),
                    pending_occurrence_at: MessageField::default(),
                },
                &record(id)
            )
            .unwrap_err(),
            RecordScheduleOccurrenceError::UnknownStateValue { value: 0 }
        );
    }
}
