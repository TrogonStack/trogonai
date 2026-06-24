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
mod tests;
