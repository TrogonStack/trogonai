use buffa::MessageField;
use trogon_decider_runtime::{Decider, Decision, WritePrecondition};
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
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use buffa::{EnumValue, MessageField};
    use trogon_decider::testing::TestCase;

    use super::*;
    use crate::commands::domain::{
        Delivery, MessageContent, Schedule as DomainSchedule, ScheduleEventStatus, ScheduleHeaders, ScheduleMessage,
    };

    fn schedule_id(id: &str) -> ScheduleId {
        ScheduleId::parse(id).unwrap()
    }

    fn create_schedule(id: &str) -> CreateSchedule {
        CreateSchedule {
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
        let command = create_schedule(id);

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

    fn paused(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::SchedulePaused {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
        }
    }

    fn resumed(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleResumed {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
        }
    }

    #[test]
    fn given_when_then_supports_create_schedule_decider() {
        TestCase::<CreateSchedule>::new()
            .given_no_history()
            .when(create_schedule("backup"))
            .then([added("backup")]);
    }

    #[test]
    fn given_when_then_supports_create_schedule_failures() {
        TestCase::<CreateSchedule>::new()
            .given([added("backup")])
            .when(create_schedule("backup"))
            .then_error(CreateScheduleDecideError::AlreadyExists {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn replaying_lifecycle_events_rejects_existing_schedule_ids() {
        TestCase::<CreateSchedule>::new()
            .given([added("backup")])
            .given([paused("backup")])
            .when(create_schedule("backup"))
            .then_error(CreateScheduleDecideError::AlreadyExists {
                id: ScheduleId::parse("backup").unwrap(),
            });

        TestCase::<CreateSchedule>::new()
            .given([added("backup")])
            .given([paused("backup")])
            .given([resumed("backup")])
            .when(create_schedule("backup"))
            .then_error(CreateScheduleDecideError::AlreadyExists {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn rejects_adding_deleted_schedule_ids() {
        TestCase::<CreateSchedule>::new()
            .given([added("backup")])
            .given([removed()])
            .when(create_schedule("backup"))
            .then_error(CreateScheduleDecideError::ScheduleDeleted {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn decide_errors_display_user_facing_messages() {
        let id = ScheduleId::parse("backup").unwrap();
        let duration_error = trogonai_proto::convert::duration_from_std(
            trogonai_proto::convert::PROTOBUF_DURATION_MAX + std::time::Duration::from_nanos(1),
        )
        .unwrap_err();

        assert_eq!(
            CreateScheduleDecideError::AlreadyExists { id: id.clone() }.to_string(),
            "schedule 'backup' already exists"
        );
        assert_eq!(
            CreateScheduleDecideError::ScheduleDeleted { id: id.clone() }.to_string(),
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
        let err = CreateScheduleDecideError::DurationConversion { source: duration_error };
        assert_eq!(
            err.to_string(),
            "schedule duration is invalid: duration must fit the protobuf Duration range: max 315576000000s, got 315576000000.000000001s"
        );
        assert!(err.source().is_some());
        assert!(
            CreateScheduleDecideError::AlreadyExists { id: id.clone() }
                .source()
                .is_none()
        );
        assert!(CreateScheduleDecideError::ScheduleDeleted { id }.source().is_none());
        assert!(CreateScheduleDecideError::MissingStateValue.source().is_none());
        assert!(
            CreateScheduleDecideError::UnknownStateValue { value: 123 }
                .source()
                .is_none()
        );
    }

    #[test]
    fn decide_rejects_invalid_state_values() {
        assert_eq!(
            CreateSchedule::decide(
                &state_v1::State {
                    state: None,
                    last_occurrence_at: MessageField::default(),
                    last_occurrence_sequence: None,
                    schedule: MessageField::default(),
                    pending_occurrence_at: MessageField::default(),
                },
                &create_schedule("backup")
            )
            .unwrap_err(),
            CreateScheduleDecideError::MissingStateValue
        );
        assert_eq!(
            CreateSchedule::decide(
                &state_v1::State {
                    state: Some(EnumValue::from(123)),
                    last_occurrence_at: MessageField::default(),
                    last_occurrence_sequence: None,
                    schedule: MessageField::default(),
                    pending_occurrence_at: MessageField::default(),
                },
                &create_schedule("backup")
            )
            .unwrap_err(),
            CreateScheduleDecideError::UnknownStateValue { value: 123 }
        );
        assert_eq!(
            CreateSchedule::decide(
                &state_v1::State {
                    state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_UNSPECIFIED)),
                    last_occurrence_at: MessageField::default(),
                    last_occurrence_sequence: None,
                    schedule: MessageField::default(),
                    pending_occurrence_at: MessageField::default(),
                },
                &create_schedule("backup")
            )
            .unwrap_err(),
            CreateScheduleDecideError::UnknownStateValue { value: 0 }
        );
    }
}
