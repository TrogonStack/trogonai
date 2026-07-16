use buffa::MessageField;
use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::convert::DurationConversionError;
use trogonai_proto::scheduler::schedules::{state_v1, v1};

use super::domain::{
    Delivery, MessageEnvelope, Schedule, ScheduleEventDelivery, ScheduleEventSchedule, ScheduleEventStatus, ScheduleId,
    ScheduleMessage,
};

#[derive(Debug, Clone)]
pub struct CreateSchedule {
    pub id: ScheduleId,
    pub status: ScheduleEventStatus,
    pub schedule: Schedule,
    pub delivery: Delivery,
    pub message: ScheduleMessage,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CreateScheduleDecideError {
    #[error("schedule '{id}' already exists")]
    AlreadyExists { id: ScheduleId },
    #[error("schedule '{id}' was deleted")]
    ScheduleDeleted { id: ScheduleId },
    #[error("schedule duration is invalid: {source}")]
    DurationConversion {
        #[source]
        source: DurationConversionError,
    },
    #[error("state value is missing")]
    MissingStateValue,
    #[error("unknown state value: {value}")]
    UnknownStateValue { value: i32 },
}

impl Decider for CreateSchedule {
    type StreamId = str;
    type State = state_v1::State;
    type Event = v1::ScheduleEvent;
    type DecideError = CreateScheduleDecideError;
    type EvolveError = super::EvolveError;

    const WRITE_PRECONDITION: Option<WritePrecondition> = Some(WritePrecondition::NoStream);

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
            return Err(CreateScheduleDecideError::MissingStateValue);
        };
        let Some(current_state) = value.as_known() else {
            return Err(CreateScheduleDecideError::UnknownStateValue { value: value.to_i32() });
        };
        match current_state {
            state_v1::StateValue::STATE_VALUE_MISSING => {
                let schedule = ScheduleEventSchedule::from(&command.schedule);
                let delivery = ScheduleEventDelivery::from(&command.delivery);

                Ok(Decision::event(v1::ScheduleEvent {
                    event: Some(
                        v1::ScheduleCreated {
                            schedule_id: command.id.as_str().to_string(),
                            status: MessageField::some(v1::ScheduleStatus::from(command.status)),
                            schedule: MessageField::some(
                                v1::Schedule::try_from(&schedule)
                                    .map_err(|source| CreateScheduleDecideError::DurationConversion { source })?,
                            ),
                            delivery: MessageField::some(
                                v1::Delivery::try_from(&delivery)
                                    .map_err(|source| CreateScheduleDecideError::DurationConversion { source })?,
                            ),
                            message: MessageField::some(v1::Message::from(&MessageEnvelope::from(&command.message))),
                        }
                        .into(),
                    ),
                }))
            }
            state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED | state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED => {
                Err(CreateScheduleDecideError::AlreadyExists { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_DELETED => {
                Err(CreateScheduleDecideError::ScheduleDeleted { id: command.id.clone() })
            }
            state_v1::StateValue::STATE_VALUE_UNSPECIFIED => {
                Err(CreateScheduleDecideError::UnknownStateValue { value: 0 })
            }
        }
    }

    fn decide_error_code(error: &Self::DecideError) -> &str {
        match error {
            CreateScheduleDecideError::AlreadyExists { .. } => "already-exists",
            CreateScheduleDecideError::ScheduleDeleted { .. } => "schedule-deleted",
            CreateScheduleDecideError::DurationConversion { .. } => "duration-conversion",
            CreateScheduleDecideError::MissingStateValue => "missing-state-value",
            CreateScheduleDecideError::UnknownStateValue { .. } => "unknown-state-value",
        }
    }
}

#[cfg(test)]
mod tests;
