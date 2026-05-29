use buffa::EnumValue;
use trogonai_proto::scheduler::schedules::{ScheduleEventCase, ScheduleStatusKind, state_v1, v1};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvolveError {
    MissingStateValue,
    UnsupportedEvent,
    UnknownStateValue { value: i32 },
}

impl std::fmt::Display for EvolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingStateValue => f.write_str("protobuf state is missing its state value"),
            Self::UnsupportedEvent => f.write_str("protobuf job event is not supported by command state"),
            Self::UnknownStateValue { value } => write!(f, "protobuf state '{value}' is unknown"),
        }
    }
}

impl std::error::Error for EvolveError {}

pub(super) fn initial_state() -> state_v1::State {
    state_v1::State {
        state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_MISSING)),
    }
}

pub(super) fn evolve(state: state_v1::State, event: &v1::ScheduleEvent) -> Result<state_v1::State, EvolveError> {
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
    let next_state = match &event.event {
        Some(ScheduleEventCase::ScheduleCreated(inner)) => {
            if current_state == state_v1::StateValue::STATE_VALUE_DELETED {
                state_v1::StateValue::STATE_VALUE_DELETED
            } else if matches!(
                inner.status.as_option().and_then(|status| status.kind.as_ref()),
                Some(ScheduleStatusKind::Paused(_))
            ) {
                state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED
            } else {
                state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED
            }
        }
        Some(ScheduleEventCase::ScheduleRemoved(_)) => state_v1::StateValue::STATE_VALUE_DELETED,
        None => return Err(EvolveError::UnsupportedEvent),
        Some(_) => return Err(EvolveError::UnsupportedEvent),
    };

    Ok(state_v1::State {
        state: Some(EnumValue::from(next_state)),
    })
}
