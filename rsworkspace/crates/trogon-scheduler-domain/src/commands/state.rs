use buffa::{EnumValue, MessageField};
use trogonai_proto::scheduler::schedules::{ScheduleEventCase, ScheduleStatusKind, state_v1, v1};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvolveError {
    #[error("protobuf state is missing its state value")]
    MissingStateValue,
    #[error("protobuf schedule created event is missing schedule")]
    MissingSchedule,
    #[error("protobuf schedule created event is missing status")]
    MissingStatus,
    #[error("protobuf schedule event is not supported by command state")]
    UnsupportedEvent,
    #[error("protobuf schedule occurrence event is missing occurrence_at")]
    MissingOccurrenceAt,
    #[error("protobuf schedule occurrence event is missing occurrence_sequence")]
    MissingOccurrenceSequence,
    #[error("protobuf state '{value}' is unknown")]
    UnknownStateValue { value: i32 },
}

pub fn initial_state() -> state_v1::State {
    state_v1::State {
        state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_MISSING)),
        last_occurrence_at: MessageField::default(),
        last_occurrence_sequence: None,
        schedule: MessageField::default(),
        pending_occurrence_at: MessageField::default(),
        completed: None,
    }
}

pub fn evolve(state: state_v1::State, event: &v1::ScheduleEvent) -> Result<state_v1::State, EvolveError> {
    let Some(value) = state.state.as_ref() else {
        return Err(EvolveError::MissingStateValue);
    };
    let Some(current_state) = value.as_known() else {
        return Err(EvolveError::UnknownStateValue { value: value.to_i32() });
    };
    let current_state = match current_state {
        state_v1::StateValue::STATE_VALUE_MISSING
        | state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED
        | state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED
        | state_v1::StateValue::STATE_VALUE_DELETED => current_state,
        state_v1::StateValue::STATE_VALUE_UNSPECIFIED => {
            return Err(EvolveError::UnknownStateValue {
                value: current_state as i32,
            });
        }
    };

    let mut last_occurrence_at = state.last_occurrence_at.clone();
    let mut last_occurrence_sequence = state.last_occurrence_sequence;
    let mut schedule = state.schedule.clone();
    let mut pending_occurrence_at = state.pending_occurrence_at.clone();
    let mut completed = state.completed;

    let next_state = match &event.event {
        Some(ScheduleEventCase::ScheduleCreated(inner)) => {
            // A creation event must carry the schedule and a resolvable status; reject malformed
            // ones rather than promoting them to a present state with empty/assumed fields.
            if inner.schedule.as_option().is_none() {
                return Err(EvolveError::MissingSchedule);
            }
            let status = inner.status.as_option().ok_or(EvolveError::MissingStatus)?;
            let status_kind = status.kind.as_ref().ok_or(EvolveError::MissingStatus)?;
            last_occurrence_at = MessageField::default();
            last_occurrence_sequence = None;
            pending_occurrence_at = MessageField::default();
            completed = None;
            schedule = inner.schedule.clone();
            if current_state == state_v1::StateValue::STATE_VALUE_DELETED {
                state_v1::StateValue::STATE_VALUE_DELETED
            } else if matches!(status_kind, ScheduleStatusKind::Paused(_)) {
                state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED
            } else {
                state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED
            }
        }
        Some(ScheduleEventCase::ScheduleRemoved(_)) => {
            pending_occurrence_at = MessageField::default();
            state_v1::StateValue::STATE_VALUE_DELETED
        }
        Some(ScheduleEventCase::SchedulePaused(_)) => {
            if current_state == state_v1::StateValue::STATE_VALUE_DELETED {
                state_v1::StateValue::STATE_VALUE_DELETED
            } else {
                state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED
            }
        }
        Some(ScheduleEventCase::ScheduleResumed(_)) => {
            if current_state == state_v1::StateValue::STATE_VALUE_DELETED {
                state_v1::StateValue::STATE_VALUE_DELETED
            } else {
                // Resume is the boundary that discards an unrecorded paused wakeup
                // so scheduling can re-arm from durable occurrence progress.
                pending_occurrence_at = MessageField::default();
                state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED
            }
        }
        Some(ScheduleEventCase::ScheduleOccurrenceRecorded(inner)) => {
            let occurrence_at = inner
                .occurrence_at
                .as_option()
                .ok_or(EvolveError::MissingOccurrenceAt)?;
            last_occurrence_at = MessageField::some(occurrence_at.clone());
            last_occurrence_sequence = Some(
                inner
                    .occurrence_sequence
                    .ok_or(EvolveError::MissingOccurrenceSequence)?,
            );
            pending_occurrence_at = MessageField::default();
            current_state
        }
        Some(ScheduleEventCase::ScheduleOccurrenceScheduled(inner)) => {
            let occurrence_at = inner
                .occurrence_at
                .as_option()
                .ok_or(EvolveError::MissingOccurrenceAt)?;
            pending_occurrence_at = MessageField::some(occurrence_at.clone());
            current_state
        }
        Some(ScheduleEventCase::ScheduleCompleted(_)) => {
            pending_occurrence_at = MessageField::default();
            completed = Some(true);
            current_state
        }
        None => return Err(EvolveError::UnsupportedEvent),
    };

    Ok(state_v1::State {
        state: Some(EnumValue::from(next_state)),
        last_occurrence_at,
        last_occurrence_sequence,
        schedule,
        pending_occurrence_at,
        completed,
    })
}

#[cfg(test)]
mod tests;
