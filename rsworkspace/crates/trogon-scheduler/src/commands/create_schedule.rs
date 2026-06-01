use buffa::MessageField;
use trogon_decider_runtime::{Decider, Decision, WritePrecondition};
use trogonai_proto::convert::DurationConversionError;
use trogonai_proto::scheduler::schedules::{state_v1, v1};

use super::domain::{
    Delivery, MessageEnvelope, Schedule, ScheduleEventDelivery, ScheduleEventSchedule, ScheduleEventStatus, ScheduleId,
    ScheduleMessage,
};

#[derive(Debug, Clone)]
pub struct CreateScheduleCommand {
    pub id: ScheduleId,
    pub status: ScheduleEventStatus,
    pub schedule: Schedule,
    pub delivery: Delivery,
    pub message: ScheduleMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateScheduleDecideError {
    AlreadyExists { id: ScheduleId },
    ScheduleDeleted { id: ScheduleId },
    DurationConversion { source: DurationConversionError },
    MissingStateValue,
    UnknownStateValue { value: i32 },
}

impl std::fmt::Display for CreateScheduleDecideError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyExists { id } => write!(formatter, "schedule '{id}' already exists"),
            Self::ScheduleDeleted { id } => write!(formatter, "schedule '{id}' was deleted"),
            Self::DurationConversion { source } => write!(formatter, "schedule duration is invalid: {source}"),
            Self::MissingStateValue => formatter.write_str("state value is missing"),
            Self::UnknownStateValue { value } => write!(formatter, "unknown state value: {value}"),
        }
    }
}

impl std::error::Error for CreateScheduleDecideError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DurationConversion { source } => Some(source),
            Self::AlreadyExists { .. }
            | Self::ScheduleDeleted { .. }
            | Self::MissingStateValue
            | Self::UnknownStateValue { .. } => None,
        }
    }
}

impl Decider for CreateScheduleCommand {
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
}

#[cfg(test)]
mod tests {
    use buffa::EnumValue;
    use trogon_decider::testing::TestCase;

    use super::*;
    use crate::commands::domain::{
        Delivery, MessageContent, Schedule as DomainSchedule, ScheduleEventStatus, ScheduleHeaders, ScheduleMessage,
    };

    fn schedule_id(id: &str) -> ScheduleId {
        ScheduleId::parse(id).unwrap()
    }

    fn create_schedule_command(id: &str) -> CreateScheduleCommand {
        CreateScheduleCommand {
            id: schedule_id(id),
            status: ScheduleEventStatus::Scheduled,
            schedule: DomainSchedule::every(std::time::Duration::from_secs(30)).unwrap(),
            delivery: Delivery::nats_event("agent.run").unwrap(),
            message: ScheduleMessage {
                content: MessageContent::json(r#"{"kind":"heartbeat"}"#),
                headers: ScheduleHeaders::default(),
            },
        }
    }

    fn added(id: &str) -> v1::ScheduleEvent {
        let command = create_schedule_command(id);

        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleCreated {
                    schedule_id: command.id.as_str().to_string(),
                    status: MessageField::some(v1::ScheduleStatus::from(command.status)),
                    schedule: MessageField::some(
                        v1::Schedule::try_from(&ScheduleEventSchedule::from(&command.schedule)).unwrap(),
                    ),
                    delivery: MessageField::some(
                        v1::Delivery::try_from(&ScheduleEventDelivery::from(&command.delivery)).unwrap(),
                    ),
                    message: MessageField::some(v1::Message::from(&MessageEnvelope::from(&command.message))),
                }
                .into(),
            ),
        }
    }

    fn removed() -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleRemoved {
                    schedule_id: "backup".to_string(),
                }
                .into(),
            ),
        }
    }

    #[test]
    fn given_when_then_supports_create_schedule_decider() {
        TestCase::<CreateScheduleCommand>::new()
            .given_no_history()
            .when(create_schedule_command("backup"))
            .then([added("backup")]);
    }

    #[test]
    fn given_when_then_supports_create_schedule_failures() {
        TestCase::<CreateScheduleCommand>::new()
            .given([added("backup")])
            .when(create_schedule_command("backup"))
            .then_error(CreateScheduleDecideError::AlreadyExists {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn rejects_adding_deleted_schedule_ids() {
        TestCase::<CreateScheduleCommand>::new()
            .given([added("backup")])
            .given([removed()])
            .when(create_schedule_command("backup"))
            .then_error(CreateScheduleDecideError::ScheduleDeleted {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn decide_errors_display_user_facing_messages() {
        let id = ScheduleId::parse("backup").unwrap();

        assert_eq!(
            CreateScheduleDecideError::AlreadyExists { id: id.clone() }.to_string(),
            "schedule 'backup' already exists"
        );
        assert_eq!(
            CreateScheduleDecideError::ScheduleDeleted { id }.to_string(),
            "schedule 'backup' was deleted"
        );
        assert_eq!(
            CreateScheduleDecideError::MissingStateValue.to_string(),
            "state value is missing"
        );
        assert_eq!(
            CreateScheduleDecideError::UnknownStateValue { value: 123 }.to_string(),
            "unknown state value: 123"
        );
    }

    #[test]
    fn decide_rejects_invalid_state_values() {
        assert_eq!(
            CreateScheduleCommand::decide(&state_v1::State { state: None }, &create_schedule_command("backup"))
                .unwrap_err(),
            CreateScheduleDecideError::MissingStateValue
        );
        assert_eq!(
            CreateScheduleCommand::decide(
                &state_v1::State {
                    state: Some(EnumValue::from(123)),
                },
                &create_schedule_command("backup")
            )
            .unwrap_err(),
            CreateScheduleDecideError::UnknownStateValue { value: 123 }
        );
        assert_eq!(
            CreateScheduleCommand::decide(
                &state_v1::State {
                    state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_UNSPECIFIED)),
                },
                &create_schedule_command("backup")
            )
            .unwrap_err(),
            CreateScheduleDecideError::UnknownStateValue { value: 0 }
        );
    }
}
