use chrono::{DateTime, Utc};
use trogon_decider_runtime::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot};
use trogonai_proto::convert::TimestampConversionError;
use trogonai_proto::scheduler::schedules::{state_v1, v1};

use super::domain::ScheduleId;
use super::rrule::{RRuleCursor, RRuleExpansionError, next_rrule_occurrence, schedule_or_complete_event};

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
        source: RRuleExpansionError,
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
                    Some(last) if last >= floor => RRuleCursor::after(last),
                    _ => RRuleCursor::at_or_after(floor),
                };

                let next_occurrence = next_rrule_occurrence(schedule, cursor)
                    .map_err(|source| ScheduleNextOccurrenceError::NextOccurrence { source })?;

                let event = schedule_or_complete_event(
                    command.id.as_str(),
                    next_occurrence,
                    last_sequence.saturating_add(1),
                    last_sequence,
                    command.now,
                );

                Ok(Decision::event(event))
            }
            state_v1::StateValue::STATE_VALUE_UNSPECIFIED => {
                Err(ScheduleNextOccurrenceError::UnknownStateValue { value: 0 })
            }
        }
    }
}

impl CommandSnapshotPolicy for ScheduleNextOccurrence {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::COMMAND_SNAPSHOT_POLICY;
}

#[cfg(test)]
mod tests {
    use buffa::{EnumValue, MessageField};
    use chrono::TimeZone;

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

    #[test]
    fn decider_identity_delegates_to_schedule_state() {
        let command = command("recurring", Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap());
        assert_eq!(command.stream_id(), "recurring");
        assert_eq!(
            ScheduleNextOccurrence::initial_state(),
            super::super::state::initial_state()
        );

        let event = v1::ScheduleEvent {
            event: Some(
                v1::ScheduleRemoved {
                    schedule_id: "recurring".to_string(),
                }
                .into(),
            ),
        };

        let evolved = ScheduleNextOccurrence::evolve(ScheduleNextOccurrence::initial_state(), &event).unwrap();
        assert_eq!(
            evolved.state.unwrap().as_known(),
            Some(state_v1::StateValue::STATE_VALUE_DELETED)
        );
    }

    #[test]
    fn arms_the_first_occurrence_for_a_created_schedule() {
        let id = "recurring";
        let state = enabled_state(None, None, MessageField::some(rrule_schedule(3)));
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();

        let decision = ScheduleNextOccurrence::decide(&state, &command(id, now)).unwrap();
        assert!(matches!(&decision, Decision::Events(_)));
        if let Decision::Events(events) = decision {
            assert_eq!(
                events.as_slice(),
                &[v1::ScheduleEvent {
                    event: Some(
                        v1::ScheduleOccurrenceScheduled {
                            schedule_id: id.to_string(),
                            occurrence_sequence: Some(1),
                            occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(
                                &Utc.with_ymd_and_hms(2026, 6, 3, 0, 0, 0).unwrap()
                            )),
                            scheduled_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&now)),
                        }
                        .into(),
                    ),
                }]
            );
        }
    }

    #[test]
    fn arms_after_the_last_recent_recorded_occurrence() {
        let id = "recurring";
        let last = Utc.with_ymd_and_hms(2026, 6, 3, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 6, 4, 0, 0, 0).unwrap();
        let state = state_v1::State {
            state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)),
            last_occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&last)),
            last_occurrence_sequence: Some(1),
            schedule: MessageField::some(rrule_schedule(4)),
            pending_occurrence_at: MessageField::default(),
        };

        let decision = ScheduleNextOccurrence::decide(&state, &command(id, now)).unwrap();
        assert!(matches!(&decision, Decision::Events(_)));
        if let Decision::Events(events) = decision {
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
    }

    #[test]
    fn completes_when_no_future_occurrence_remains() {
        let id = "recurring";
        let state = enabled_state(None, None, MessageField::some(rrule_schedule(2)));
        let now = Utc.with_ymd_and_hms(2026, 6, 10, 0, 0, 0).unwrap();

        let decision = ScheduleNextOccurrence::decide(&state, &command(id, now)).unwrap();
        assert!(matches!(&decision, Decision::Events(_)));
        if let Decision::Events(events) = decision {
            assert_eq!(
                events.as_slice(),
                &[v1::ScheduleEvent {
                    event: Some(
                        v1::ScheduleCompleted {
                            schedule_id: id.to_string(),
                            last_occurrence_sequence: Some(0),
                        }
                        .into(),
                    ),
                }]
            );
        }
    }

    #[test]
    fn rejects_when_already_armed() {
        let id = "recurring";
        let pending = Utc.with_ymd_and_hms(2026, 6, 3, 0, 0, 0).unwrap();
        let state = enabled_state(None, Some(pending), MessageField::some(rrule_schedule(3)));
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();

        assert_eq!(
            ScheduleNextOccurrence::decide(&state, &command(id, now)).unwrap_err(),
            ScheduleNextOccurrenceError::AlreadyArmed { id: schedule_id(id) }
        );
    }

    #[test]
    fn rejects_paused_deleted_and_missing_schedules() {
        let id = "recurring";
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();

        for (value, expected) in [
            (
                state_v1::StateValue::STATE_VALUE_MISSING,
                ScheduleNextOccurrenceError::ScheduleNotFound { id: schedule_id(id) },
            ),
            (
                state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED,
                ScheduleNextOccurrenceError::SchedulePaused { id: schedule_id(id) },
            ),
            (
                state_v1::StateValue::STATE_VALUE_DELETED,
                ScheduleNextOccurrenceError::ScheduleDeleted { id: schedule_id(id) },
            ),
        ] {
            let state = state_v1::State {
                state: Some(EnumValue::from(value)),
                last_occurrence_at: MessageField::default(),
                last_occurrence_sequence: None,
                schedule: MessageField::some(rrule_schedule(3)),
                pending_occurrence_at: MessageField::default(),
            };
            assert_eq!(
                ScheduleNextOccurrence::decide(&state, &command(id, now)).unwrap_err(),
                expected
            );
        }
    }

    #[test]
    fn rejects_when_schedule_definition_is_missing() {
        let id = "recurring";
        let state = enabled_state(None, None, MessageField::default());
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();

        assert_eq!(
            ScheduleNextOccurrence::decide(&state, &command(id, now)).unwrap_err(),
            ScheduleNextOccurrenceError::MissingSchedule { id: schedule_id(id) }
        );
    }

    #[test]
    fn rejects_invalid_last_recorded_timestamp() {
        let id = "recurring";
        let state = state_v1::State {
            state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)),
            last_occurrence_at: MessageField::some(buffa_types::google::protobuf::Timestamp {
                seconds: i64::MAX,
                nanos: 0,
                ..Default::default()
            }),
            last_occurrence_sequence: Some(1),
            schedule: MessageField::some(rrule_schedule(3)),
            pending_occurrence_at: MessageField::default(),
        };
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();

        assert!(matches!(
            ScheduleNextOccurrence::decide(&state, &command(id, now)).unwrap_err(),
            ScheduleNextOccurrenceError::LastRecordedAt { .. }
        ));
    }

    #[test]
    fn rejects_malformed_state_values() {
        let id = "recurring";
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();

        assert_eq!(
            ScheduleNextOccurrence::decide(
                &state_v1::State {
                    state: None,
                    last_occurrence_at: MessageField::default(),
                    last_occurrence_sequence: None,
                    schedule: MessageField::default(),
                    pending_occurrence_at: MessageField::default(),
                },
                &command(id, now)
            )
            .unwrap_err(),
            ScheduleNextOccurrenceError::MissingStateValue
        );
        assert_eq!(
            ScheduleNextOccurrence::decide(
                &state_v1::State {
                    state: Some(EnumValue::from(123)),
                    last_occurrence_at: MessageField::default(),
                    last_occurrence_sequence: None,
                    schedule: MessageField::default(),
                    pending_occurrence_at: MessageField::default(),
                },
                &command(id, now)
            )
            .unwrap_err(),
            ScheduleNextOccurrenceError::UnknownStateValue { value: 123 }
        );
        assert_eq!(
            ScheduleNextOccurrence::decide(
                &state_v1::State {
                    state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_UNSPECIFIED)),
                    last_occurrence_at: MessageField::default(),
                    last_occurrence_sequence: None,
                    schedule: MessageField::default(),
                    pending_occurrence_at: MessageField::default(),
                },
                &command(id, now)
            )
            .unwrap_err(),
            ScheduleNextOccurrenceError::UnknownStateValue { value: 0 }
        );
    }
}
