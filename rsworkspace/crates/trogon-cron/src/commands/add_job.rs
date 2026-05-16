use buffa::MessageField;
use trogon_cron_jobs_proto::{state_v1, v1};
use trogon_eventsourcing::{CommandSnapshotPolicy, Decider, Decision, FrequencySnapshot, WritePrecondition};

use super::domain::{Job, JobId};

#[derive(Debug, Clone)]
pub struct AddJobCommand {
    pub job: Job,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddJobDecideError {
    AlreadyExists { id: JobId },
    JobDeleted { id: JobId },
    MissingStateValue,
    UnknownStateValue { value: i32 },
}

impl std::fmt::Display for AddJobDecideError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for AddJobDecideError {}

impl AddJobCommand {
    pub const fn new(job: Job) -> Self {
        Self { job }
    }
}

impl Decider for AddJobCommand {
    type StreamId = str;
    type State = state_v1::State;
    type Event = v1::JobEvent;
    type DecideError = AddJobDecideError;
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
            return Err(AddJobDecideError::MissingStateValue);
        };
        let Some(current_state) = value.as_known() else {
            return Err(AddJobDecideError::UnknownStateValue { value: value.to_i32() });
        };
        match current_state {
            state_v1::StateValue::STATE_VALUE_MISSING => Ok(Decision::event(v1::JobEvent {
                event: Some(
                    v1::JobAdded {
                        job: MessageField::some(v1::JobDetails::from(&command.job)),
                    }
                    .into(),
                ),
            })),
            state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED | state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED => {
                Err(AddJobDecideError::AlreadyExists {
                    id: command.job.id.clone(),
                })
            }
            state_v1::StateValue::STATE_VALUE_DELETED => Err(AddJobDecideError::JobDeleted {
                id: command.job.id.clone(),
            }),
            state_v1::StateValue::STATE_VALUE_UNSPECIFIED => Err(AddJobDecideError::UnknownStateValue { value: 0 }),
        }
    }
}

impl CommandSnapshotPolicy for AddJobCommand {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::COMMAND_SNAPSHOT_POLICY;
}

#[cfg(test)]
mod tests {
    use trogon_decider::testing::TestCase;
    use trogon_eventsourcing::snapshot::SnapshotSchema;
    use trogon_eventsourcing::{CommandExecution, CommandFailure, Events, run_task_immediately};

    use super::*;
    use crate::commands::domain::{Delivery, JobHeaders, JobMessage, JobStatus, MessageContent, Schedule};
    use crate::{
        CronJob, GetJobCommand, JobEventDelivery, JobEventSchedule, JobEventStatus,
        MessageContent as ReadMessageContent, MessageEnvelope, MessageHeaders, mocks::MockCronStore,
    };

    fn job_id(id: &str) -> JobId {
        JobId::parse(id).unwrap()
    }

    fn job(id: &str) -> Job {
        Job {
            id: job_id(id),
            status: JobStatus::Enabled,
            schedule: Schedule::every(30).unwrap(),
            delivery: Delivery::nats_event("agent.run").unwrap(),
            message: JobMessage {
                content: MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
                headers: JobHeaders::default(),
            },
        }
    }

    fn expected_job(id: &str) -> CronJob {
        CronJob {
            id: id.to_string(),
            status: JobEventStatus::Enabled,
            schedule: JobEventSchedule::Every { every_sec: 30 },
            delivery: JobEventDelivery::NatsEvent {
                route: "agent.run".to_string(),
                ttl_sec: None,
                source: None,
            },
            message: MessageEnvelope {
                content: ReadMessageContent::from_static(r#"{"kind":"heartbeat"}"#),
                headers: MessageHeaders::default(),
            },
        }
    }

    fn added(id: &str) -> v1::JobEvent {
        v1::JobEvent {
            event: Some(
                v1::JobAdded {
                    job: MessageField::some(v1::JobDetails::from(&job(id))),
                }
                .into(),
            ),
        }
    }

    fn removed() -> v1::JobEvent {
        v1::JobEvent {
            event: Some(v1::JobRemoved {}.into()),
        }
    }

    #[test]
    fn given_when_then_supports_register_job_decider() {
        TestCase::<AddJobCommand>::new()
            .given_no_history()
            .when(AddJobCommand::new(job("backup")))
            .then(trogon_decider::events![added("backup")]);
    }

    #[test]
    fn given_when_then_supports_register_job_failures() {
        TestCase::<AddJobCommand>::new()
            .given([added("backup")])
            .when(AddJobCommand::new(job("backup")))
            .then_error(AddJobDecideError::AlreadyExists {
                id: JobId::parse("backup").unwrap(),
            });
    }

    #[test]
    fn rejects_adding_deleted_job_ids() {
        TestCase::<AddJobCommand>::new()
            .given([added("backup")])
            .given([removed()])
            .when(AddJobCommand::new(job("backup")))
            .then_error(AddJobDecideError::JobDeleted {
                id: JobId::parse("backup").unwrap(),
            });
    }

    #[tokio::test]
    async fn run_registers_job_in_store() {
        let store = MockCronStore::new();

        let outcome = CommandExecution::new(&store, &AddJobCommand::new(job("backup")))
            .with_snapshot(&store)
            .with_task_runtime(run_task_immediately)
            .execute()
            .await
            .unwrap();
        assert_eq!(outcome.stream_position.get(), 1);
        assert_eq!(outcome.events, Events::one(added("backup")));

        let stored_job = store
            .get_job(GetJobCommand::new(crate::JobId::parse("backup").unwrap()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored_job, expected_job("backup"));

        let command_snapshot = store
            .read_command_snapshot::<state_v1::State>(
                state_v1::State::snapshot_store_config(),
                &JobId::parse("backup").unwrap(),
            )
            .unwrap();
        assert!(command_snapshot.is_none());
    }

    #[tokio::test]
    async fn run_rejects_adding_existing_job_with_domain_error() {
        let store = MockCronStore::new();

        CommandExecution::new(&store, &AddJobCommand::new(job("backup")))
            .with_snapshot(&store)
            .with_task_runtime(run_task_immediately)
            .execute()
            .await
            .unwrap();

        let error = CommandExecution::new(&store, &AddJobCommand::new(job("backup")))
            .with_snapshot(&store)
            .with_task_runtime(run_task_immediately)
            .execute()
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Decide(AddJobDecideError::AlreadyExists { ref id })
                if id.to_string() == "backup"
        ));
    }

    #[tokio::test]
    async fn run_rejects_adding_deleted_job_id() {
        let store = MockCronStore::new();

        CommandExecution::new(&store, &AddJobCommand::new(job("backup")))
            .with_snapshot(&store)
            .with_task_runtime(run_task_immediately)
            .execute()
            .await
            .unwrap();
        CommandExecution::new(&store, &crate::RemoveJobCommand::new(JobId::parse("backup").unwrap()))
            .with_snapshot(&store)
            .with_task_runtime(run_task_immediately)
            .execute()
            .await
            .unwrap();

        let error = CommandExecution::new(&store, &AddJobCommand::new(job("backup")))
            .with_snapshot(&store)
            .with_task_runtime(run_task_immediately)
            .execute()
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            CommandFailure::Decide(AddJobDecideError::JobDeleted { ref id })
                if id.to_string() == "backup"
        ));
    }
}
