use buffa::MessageField;
use trogon_decider_runtime::{Decider, Decision, WritePrecondition};
use trogonai_proto::scheduler::schedules::{state_v1, v1};

use super::domain::{Job, ScheduleDetails, ScheduleId};

fn schedule_created_from_job(job: &Job) -> v1::ScheduleCreated {
    let details = ScheduleDetails::from(job);

    v1::ScheduleCreated {
        schedule_id: job.id.as_str().to_string(),
        status: MessageField::some(v1::ScheduleStatus::from(details.status)),
        schedule: MessageField::some(v1::Schedule::from(&details.schedule)),
        delivery: MessageField::some(v1::Delivery::from(&details.delivery)),
        message: MessageField::some(v1::Message::from(&details.message)),
    }
}

#[derive(Debug, Clone)]
pub struct CreateScheduleCommand {
    pub job: Job,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateScheduleDecideError {
    AlreadyExists { id: ScheduleId },
    JobDeleted { id: ScheduleId },
    MissingStateValue,
    UnknownStateValue { value: i32 },
}

impl std::fmt::Display for CreateScheduleDecideError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for CreateScheduleDecideError {}

impl CreateScheduleCommand {
    pub fn new(job: Job) -> Self {
        Self { job }
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
        self.job.id.as_str()
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
            state_v1::StateValue::STATE_VALUE_MISSING => Ok(Decision::event(v1::ScheduleEvent {
                event: Some(schedule_created_from_job(&command.job).into()),
            })),
            state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED | state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED => {
                Err(CreateScheduleDecideError::AlreadyExists {
                    id: command.job.id.clone(),
                })
            }
            state_v1::StateValue::STATE_VALUE_DELETED => Err(CreateScheduleDecideError::JobDeleted {
                id: command.job.id.clone(),
            }),
            state_v1::StateValue::STATE_VALUE_UNSPECIFIED => {
                Err(CreateScheduleDecideError::UnknownStateValue { value: 0 })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use trogon_decider::testing::TestCase;

    use super::*;
    use crate::commands::domain::{
        Delivery, JobHeaders, JobMessage, JobStatus, MessageContent, Schedule as DomainSchedule,
    };

    fn job_id(id: &str) -> ScheduleId {
        ScheduleId::parse(id).unwrap()
    }

    fn create_job_command(id: &str) -> CreateScheduleCommand {
        CreateScheduleCommand::new(job(id))
    }

    fn job(id: &str) -> Job {
        Job {
            id: job_id(id),
            status: JobStatus::Enabled,
            schedule: DomainSchedule::every(30).unwrap(),
            delivery: Delivery::nats_event("agent.run").unwrap(),
            message: JobMessage {
                content: MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
                headers: JobHeaders::default(),
            },
        }
    }

    fn added(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(schedule_created_from_job(&job(id)).into()),
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
    fn given_when_then_supports_register_job_decider() {
        TestCase::<CreateScheduleCommand>::new()
            .given_no_history()
            .when(create_job_command("backup"))
            .then([added("backup")]);
    }

    #[test]
    fn given_when_then_supports_register_job_failures() {
        TestCase::<CreateScheduleCommand>::new()
            .given([added("backup")])
            .when(create_job_command("backup"))
            .then_error(CreateScheduleDecideError::AlreadyExists {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn rejects_adding_deleted_job_ids() {
        TestCase::<CreateScheduleCommand>::new()
            .given([added("backup")])
            .given([removed()])
            .when(create_job_command("backup"))
            .then_error(CreateScheduleDecideError::JobDeleted {
                id: ScheduleId::parse("backup").unwrap(),
            });
    }
}
